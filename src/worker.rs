use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fmt::Write as _;
use std::future::Future;
use std::io;
use std::path::{Component, Path};
use std::pin::Pin;
use std::sync::{Arc, RwLock};
use std::time::Duration as StdDuration;

use chrono::{DateTime, Duration, NaiveDateTime, Utc};
use http::StatusCode;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Transaction};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, watch};
use tokio::task::JoinSet;
use url::Url;

use crate::config::WorkerConfig;
use crate::jobs::{
    ACTIVITYPUB_ACCOUNT_DELETE_JOB_KIND, ACTIVITYPUB_ACCOUNT_UPDATE_JOB_KIND,
    ACTIVITYPUB_ANNOUNCE_RESOLVE_JOB_KIND, ACTIVITYPUB_DELIVERY_JOB_KIND,
    ACTIVITYPUB_EMOJI_CLEANUP_JOB_KIND, ACTIVITYPUB_EMOJI_FETCH_JOB_KIND,
    ACTIVITYPUB_INBOX_JOB_KIND, ACTIVITYPUB_MEDIA_FETCH_JOB_KIND,
    ACTIVITYPUB_NOTE_RESOLVE_JOB_KIND, ACTIVITYPUB_STATUS_DISTRIBUTION_JOB_KIND,
    ACTIVITYPUB_THREAD_RESOLVE_JOB_KIND, ClaimedJob, JobError, JobSpec,
    LOCAL_MEDIA_CLEANUP_JOB_KIND, Lane, MASTODON_ACCOUNT_PURGE_JOB_KIND,
    MASTODON_DOMAIN_BLOCK_JOB_KIND, MASTODON_DOMAIN_PURGE_JOB_KIND, NOTIFICATION_CLEANUP_JOB_KIND,
    NOTIFICATION_CREATE_JOB_KIND, NOTIFICATION_UNFILTER_JOB_KIND, Queue, WorkerHeartbeat,
    record_outbox_once_in, record_stream_event_in,
};
use crate::mail::MailRuntime;
use crate::mastodon::activitypub_inbox::{
    InboxActivity, InboxJob, parse_activity, parse_job_arguments, validate_note_object,
};
use crate::mastodon::{
    Account, AccountPurgeOutcome, HttpSignatureSigner, NotificationActivity, NotificationCreate,
    RemoteFollowOutcome, RemoteUndoReferenceKind, Repository, STATUS_NOTIFICATION_JOB_KIND,
    StatusVisibility, WriteError, WriteRepository, activitypub,
};
use crate::paperclip::{
    PaperclipAttachment, PaperclipMetadata, PaperclipRoot, parse_paperclip_path,
    prepare_custom_emoji, prepare_media_attachment, write_prepared_custom_emoji,
    write_prepared_media,
};
use crate::remote::{
    RemoteAccountResolver, RemoteFetchError, RemoteFetchLimits, RemoteFetcher,
    canonical_remote_domain, canonical_remote_domain_from_url,
};
use crate::streaming::event_logical_key;

type HandlerFuture = Pin<Box<dyn Future<Output = Result<(), HandlerFailure>> + Send>>;
type HandlerFn = dyn Fn(ClaimedJob) -> HandlerFuture + Send + Sync;
const THREAD_ACTIVITYPUB_CONTENT_TYPES: &[&str] = &[
    "application/activity+json",
    "application/ld+json; profile=\"https://www.w3.org/ns/activitystreams\"",
];
const REMOTE_MEDIA_CONTENT_TYPES: &[&str] = &["image/jpeg", "image/png", "image/gif", "image/webp"];
const REMOTE_MEDIA_MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const NOTIFICATION_CLEANUP_BATCH_SIZE: i64 = 1_000;

async fn activitypub_quote_parts(
    repository: &Repository,
    config: &ActivityPubDeliveryConfig,
    status_id: i64,
) -> Result<(Option<String>, Option<String>, Option<String>), HandlerFailure> {
    let Some(target) = repository
        .activitypub_quote_target(status_id)
        .await
        .map_err(|_| HandlerFailure::retry("status quote lookup failed"))?
    else {
        return Ok((None, None, None));
    };
    let quoted_url = if target.local {
        Some(
            config
                .origin
                .join(&format!("@{}/{}", target.username, target.id))
                .expect("worker origin is absolute")
                .to_string(),
        )
    } else {
        target
            .url
            .clone()
            .filter(|url| !crate::paperclip::rails_blank(url))
    };
    let quoted_identifier = target.uri.clone().or_else(|| {
        if target.local {
            Some(activitypub::local_status_uri(
                &config.origin,
                target.account_id,
                &target.username,
                target.id_scheme,
                target.id,
            ))
        } else {
            target
                .url
                .clone()
                .filter(|url| !crate::paperclip::rails_blank(url))
        }
    });
    let quote_authorization = quoted_identifier.as_ref().and_then(|_| {
        if target.quoted_account_local {
            Some(activitypub::local_quote_authorization_url(
                &config.origin,
                target.account_id,
                &target.username,
                target.id_scheme,
                target.quote_id,
            ))
        } else {
            target
                .approval_uri
                .filter(|uri| !crate::paperclip::rails_blank(uri))
        }
    });
    Ok((quoted_url, quoted_identifier, quote_authorization))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResourceClass {
    None,
    RemoteHttp,
    Media,
}

#[derive(Clone, Debug)]
pub struct ActivityPubDeliveryConfig {
    pub origin: Url,
    pub local_domain: String,
    pub media_root_url: String,
    pub media_root: Option<PaperclipRoot>,
    pub limited_federation: bool,
    #[cfg(feature = "test-support")]
    pub remote_media_endpoint: Option<std::net::SocketAddr>,
    #[cfg(feature = "test-support")]
    pub remote_delivery_endpoint: Option<std::net::SocketAddr>,
    #[cfg(feature = "test-support")]
    pub remote_fetch_endpoint: Option<std::net::SocketAddr>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FailureDisposition {
    Retry,
    Permanent,
}

#[derive(Clone, Debug)]
pub struct HandlerFailure {
    disposition: FailureDisposition,
    message: String,
}

impl HandlerFailure {
    #[must_use]
    pub fn retry(message: impl Into<String>) -> Self {
        Self {
            disposition: FailureDisposition::Retry,
            message: message.into(),
        }
    }

    #[must_use]
    pub fn permanent(message: impl Into<String>) -> Self {
        Self {
            disposition: FailureDisposition::Permanent,
            message: message.into(),
        }
    }
}

impl fmt::Display for HandlerFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for HandlerFailure {}

struct Handler {
    lane: Lane,
    resource: ResourceClass,
    run: Arc<HandlerFn>,
}

impl Clone for Handler {
    fn clone(&self) -> Self {
        Self {
            lane: self.lane,
            resource: self.resource,
            run: Arc::clone(&self.run),
        }
    }
}

#[derive(Clone, Default)]
pub struct HandlerRegistry {
    handlers: Arc<RwLock<BTreeMap<String, Handler>>>,
}

impl HandlerRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers exactly one handler for a durable job kind.
    ///
    /// # Errors
    ///
    /// Rejects empty/oversized kinds and duplicate registrations.
    pub fn register<F, Fut>(
        &self,
        kind: impl Into<String>,
        lane: Lane,
        resource: ResourceClass,
        handler: F,
    ) -> Result<(), WorkerError>
    where
        F: Fn(ClaimedJob) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), HandlerFailure>> + Send + 'static,
    {
        let kind = kind.into();
        if !(1..=128).contains(&kind.len()) {
            return Err(WorkerError::InvalidConfiguration(
                "handler kind must contain 1-128 bytes",
            ));
        }
        let run = Arc::new(move |job| Box::pin(handler(job)) as HandlerFuture);
        let mut handlers = self
            .handlers
            .write()
            .map_err(|_| WorkerError::RegistryUnavailable)?;
        if handlers.contains_key(&kind) {
            return Err(WorkerError::InvalidConfiguration(
                "durable job handler is already registered",
            ));
        }
        handlers.insert(
            kind,
            Handler {
                lane,
                resource,
                run,
            },
        );
        Ok(())
    }

    fn get(&self, kind: &str) -> Result<Option<Handler>, WorkerError> {
        Ok(self
            .handlers
            .read()
            .map_err(|_| WorkerError::RegistryUnavailable)?
            .get(kind)
            .cloned())
    }

    fn supported_lanes(&self) -> Result<BTreeSet<Lane>, WorkerError> {
        Ok(self
            .handlers
            .read()
            .map_err(|_| WorkerError::RegistryUnavailable)?
            .values()
            .map(|handler| handler.lane)
            .collect())
    }
}

#[derive(Debug)]
pub enum WorkerError {
    Jobs(JobError),
    InvalidConfiguration(&'static str),
    RegistryUnavailable,
    SemaphoreClosed,
    TaskFailed,
    ShutdownTimedOut,
}

impl fmt::Display for WorkerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Jobs(error) => error.fmt(formatter),
            Self::InvalidConfiguration(message) => formatter.write_str(message),
            Self::RegistryUnavailable => {
                formatter.write_str("worker handler registry is unavailable")
            }
            Self::SemaphoreClosed => formatter.write_str("worker resource limiter is closed"),
            Self::TaskFailed => formatter.write_str("a worker task stopped unexpectedly"),
            Self::ShutdownTimedOut => {
                formatter.write_str("worker shutdown exceeded its configured deadline")
            }
        }
    }
}

impl std::error::Error for WorkerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Jobs(error) => Some(error),
            Self::InvalidConfiguration(_)
            | Self::RegistryUnavailable
            | Self::SemaphoreClosed
            | Self::TaskFailed
            | Self::ShutdownTimedOut => None,
        }
    }
}

impl From<JobError> for WorkerError {
    fn from(error: JobError) -> Self {
        Self::Jobs(error)
    }
}

#[derive(Clone)]
pub struct WorkerExecutor {
    queue: Queue,
    handlers: HandlerRegistry,
    remote_http: Arc<Semaphore>,
    media: Arc<Semaphore>,
}

impl WorkerExecutor {
    /// Creates an executor with independent remote HTTP and media limits.
    ///
    /// # Errors
    ///
    /// Rejects zero concurrency limits.
    pub fn new(
        queue: Queue,
        handlers: HandlerRegistry,
        remote_http_concurrency: usize,
        media_concurrency: usize,
    ) -> Result<Self, WorkerError> {
        if remote_http_concurrency == 0 || media_concurrency == 0 {
            return Err(WorkerError::InvalidConfiguration(
                "worker resource concurrency must be positive",
            ));
        }
        Ok(Self {
            queue,
            handlers,
            remote_http: Arc::new(Semaphore::new(remote_http_concurrency)),
            media: Arc::new(Semaphore::new(media_concurrency)),
        })
    }

    /// Claims and processes at most one job, returning whether a due job existed.
    ///
    /// # Errors
    ///
    /// Returns an error when queue transitions or resource acquisition fail.
    pub async fn process_one(
        &self,
        lease_owner: &str,
        lanes: &[Lane],
        lease_duration: Duration,
    ) -> Result<bool, WorkerError> {
        let Some(job) = self.queue.claim(lease_owner, lanes, lease_duration).await? else {
            return Ok(false);
        };
        let Some(handler) = self.handlers.get(&job.kind)? else {
            self.queue
                .dead_letter(&job, "no handler is registered for this job kind")
                .await?;
            return Ok(true);
        };
        if handler.lane != job.lane {
            self.queue
                .dead_letter(&job, "job kind is registered for a different lane")
                .await?;
            return Ok(true);
        }
        let executor = self.clone();
        let run_job = job.clone();
        let mut future = Box::pin(async move {
            let _permit = executor.resource_permit(handler.resource).await?;
            Ok::<_, WorkerError>((handler.run)(run_job).await)
        });
        let renewal_milliseconds = (lease_duration.num_milliseconds() / 3).max(1);
        let renewal = StdDuration::from_millis(
            u64::try_from(renewal_milliseconds)
                .map_err(|_| WorkerError::InvalidConfiguration("lease duration is too large"))?,
        );
        let mut ticker = tokio::time::interval_at(tokio::time::Instant::now() + renewal, renewal);
        let outcome = loop {
            tokio::select! {
                result = &mut future => break Some(result?),
                _ = ticker.tick() => {
                    if !self.queue.renew(
                        job.id,
                        &job.lease_owner,
                        job.generation,
                        lease_duration,
                    ).await? {
                        // Dropping the future stops a permit waiter or handler that lost its fence
                        // from continuing work under a stale lease.
                        break None;
                    }
                }
            }
        };
        match outcome {
            None => {}
            Some(Ok(())) => {
                // External acceptance precedes this fence; acknowledgement failure must leave the
                // durable job reclaimable.
                self.queue
                    .complete(job.id, &job.lease_owner, job.generation)
                    .await?;
            }
            Some(Err(failure)) if failure.disposition == FailureDisposition::Permanent => {
                self.queue.dead_letter(&job, &failure.message).await?;
            }
            Some(Err(failure)) => {
                self.queue
                    .retry(
                        &job,
                        Utc::now() + retry_delay(job.id, job.attempt),
                        &failure.message,
                    )
                    .await?;
            }
        }
        Ok(true)
    }

    async fn resource_permit(
        &self,
        resource: ResourceClass,
    ) -> Result<Option<OwnedSemaphorePermit>, WorkerError> {
        match resource {
            ResourceClass::None => Ok(None),
            ResourceClass::RemoteHttp => Arc::clone(&self.remote_http)
                .acquire_owned()
                .await
                .map(Some)
                .map_err(|_| WorkerError::SemaphoreClosed),
            ResourceClass::Media => Arc::clone(&self.media)
                .acquire_owned()
                .await
                .map(Some)
                .map_err(|_| WorkerError::SemaphoreClosed),
        }
    }
}

#[allow(clippy::too_many_lines)]
async fn distribute_status(
    pool: PgPool,
    config: &ActivityPubDeliveryConfig,
    status_id: i64,
    activity_type: &str,
    edited_at_micros: Option<i64>,
    explicit_recipient_ids: &[i64],
) -> Result<(), HandlerFailure> {
    let is_delete = match activity_type {
        "Create" | "Update" => false,
        "Delete" => true,
        _ => {
            return Err(HandlerFailure::permanent(
                "status distribution activity type is unsupported",
            ));
        }
    };
    let requested_edited_at = edited_at_micros
        .map(|value| {
            DateTime::<Utc>::from_timestamp_micros(value)
                .map(|timestamp| timestamp.naive_utc())
                .ok_or_else(|| HandlerFailure::permanent("status edit timestamp is invalid"))
        })
        .transpose()?;
    let repository = Repository::from_pool(pool.clone());
    let status_result = if is_delete {
        repository.status_including_deleted(status_id).await
    } else {
        repository.status(status_id).await
    };
    let Some(status) =
        status_result.map_err(|_| HandlerFailure::retry("status delivery lookup failed"))?
    else {
        return Ok(());
    };
    if status.local != Some(true) {
        return Ok(());
    }
    if status.reblog_of_id.is_some() && activity_type == "Update" {
        return Ok(());
    }
    if activity_type == "Update" {
        // A newer transactional Update job owns the current payload; avoid serializing an older
        // edit with newer status content when workers fall behind rapid edits.
        let current_edited_at = status.edited_at.unwrap_or(status.updated_at);
        if requested_edited_at.is_some_and(|requested| requested != current_edited_at) {
            return Ok(());
        }
    }
    let update_version_micros = (activity_type == "Update").then(|| {
        requested_edited_at
            .or(status.edited_at)
            .unwrap_or(status.updated_at)
            .and_utc()
            .timestamp_micros()
    });
    let account = repository
        .account(status.account_id)
        .await
        .map_err(|_| HandlerFailure::retry("status author lookup failed"))?
        .ok_or_else(|| HandlerFailure::permanent("status author is missing"))?;
    let media = if is_delete {
        Vec::new()
    } else {
        repository
            .media_attachments(status_id)
            .await
            .map_err(|_| HandlerFailure::retry("status media lookup failed"))?
    };
    let status_stat = if is_delete {
        None
    } else {
        repository
            .status_stat(status_id)
            .await
            .map_err(|_| HandlerFailure::retry("status statistics lookup failed"))?
    };
    let mention_rows = repository
        .mentions(status_id)
        .await
        .map_err(|_| HandlerFailure::retry("status mention lookup failed"))?;
    let mut mentions = Vec::with_capacity(mention_rows.len());
    let mut mentioned_recipient_ids = Vec::new();
    for mention in mention_rows {
        if let Some(target) = repository
            .account(mention.account_id)
            .await
            .map_err(|_| HandlerFailure::retry("status mention target lookup failed"))?
        {
            if target.domain.is_some() && target.protocol.0 != 1 {
                continue;
            }
            if target.domain.is_some() {
                mentioned_recipient_ids.push(target.id);
            }
            if !mention.silent {
                mentions.push((mention, target));
            }
        }
    }
    let activity = if let Some(reblog_of_id) = status.reblog_of_id {
        let target_status = repository
            .status_including_deleted(reblog_of_id)
            .await
            .map_err(|_| HandlerFailure::retry("reblog target lookup failed"))?
            .ok_or_else(|| HandlerFailure::permanent("reblog target is missing"))?;
        let target_account = repository
            .account(target_status.account_id)
            .await
            .map_err(|_| HandlerFailure::retry("reblog target author lookup failed"))?
            .ok_or_else(|| HandlerFailure::permanent("reblog target author is missing"))?;
        let actor_uri = activitypub::actor_url(&config.origin, &account);
        let announce_uri = activitypub::status_uri(&config.origin, &account, &status);
        let object_uri = activitypub::status_uri(&config.origin, &target_account, &target_status);
        let (to, mut cc) = activitypub::local_announce_audience(status.visibility, &actor_uri);
        if let Value::Array(values) = &mut cc {
            values.push(Value::String(activitypub::actor_url(
                &config.origin,
                &target_account,
            )));
        }
        if is_delete {
            let undo_uri = format!("{actor_uri}#announces/{}/undo", status.id);
            activitypub::undo_announce_with_uris(
                &undo_uri,
                &actor_uri,
                &announce_uri,
                status.created_at,
                &object_uri,
                to,
                cc,
            )
        } else {
            let object = if status.visibility == StatusVisibility::Private
                && target_status.local == Some(true)
                && target_account.id == account.id
            {
                let target_media = repository
                    .media_attachments(reblog_of_id)
                    .await
                    .map_err(|_| HandlerFailure::retry("private boost media lookup failed"))?;
                let target_mention_rows = repository
                    .mentions(reblog_of_id)
                    .await
                    .map_err(|_| HandlerFailure::retry("private boost mention lookup failed"))?;
                let mut target_mentions = Vec::with_capacity(target_mention_rows.len());
                for mention in target_mention_rows {
                    if mention.silent {
                        continue;
                    }
                    if let Some(target) = repository
                        .account(mention.account_id)
                        .await
                        .map_err(|_| HandlerFailure::retry("private boost target lookup failed"))?
                    {
                        if target.domain.is_some() && target.protocol.0 != 1 {
                            continue;
                        }
                        target_mentions.push((mention, target));
                    }
                }
                let target_hashtags = repository
                    .tags(reblog_of_id)
                    .await
                    .map_err(|_| HandlerFailure::retry("private boost hashtag lookup failed"))?
                    .into_iter()
                    .map(|tag| (tag.name, tag.display_name.unwrap_or_default()))
                    .collect::<Vec<_>>();
                let target_emojis = repository
                    .activitypub_status_emojis(reblog_of_id)
                    .await
                    .map_err(|_| HandlerFailure::retry("private boost emoji lookup failed"))?;
                let target_stat = repository
                    .status_stat(reblog_of_id)
                    .await
                    .map_err(|_| HandlerFailure::retry("private boost statistics lookup failed"))?;
                let (quoted_link, quoted_identifier, quote_authorization) =
                    activitypub_quote_parts(&repository, config, reblog_of_id).await?;
                activitypub::note(
                    &config.origin,
                    &config.local_domain,
                    &target_status,
                    &target_account,
                    &config.media_root_url,
                    &target_media,
                    &target_mentions,
                    &target_hashtags,
                    &target_emojis,
                    quoted_link.as_deref(),
                    None,
                    None,
                    None,
                    quoted_identifier.as_deref(),
                    quote_authorization.as_deref(),
                    None,
                    target_stat
                        .as_ref()
                        .map_or(0, |stats| stats.favourites_count),
                    target_stat.as_ref().map_or(0, |stats| stats.reblogs_count),
                )
            } else {
                Value::String(object_uri)
            };
            activitypub::announce_with_object(
                &announce_uri,
                &actor_uri,
                status.created_at,
                object,
                to,
                cc,
            )
        }
    } else if is_delete {
        let object_uri = activitypub::status_uri(&config.origin, &account, &status);
        let delete_uri = format!("{object_uri}#delete");
        let atom_uri = status.uri.as_deref().unwrap_or(&object_uri);
        activitypub::delete_with_uris(
            &delete_uri,
            &activitypub::actor_url(&config.origin, &account),
            &object_uri,
            atom_uri,
        )
    } else {
        let hashtags = repository
            .tags(status_id)
            .await
            .map_err(|_| HandlerFailure::retry("status hashtag lookup failed"))?
            .into_iter()
            .map(|tag| (tag.name, tag.display_name.unwrap_or_default()))
            .collect::<Vec<_>>();
        let (in_reply_to_url, in_reply_to_atom_uri) = if let Some(parent_id) = status.in_reply_to_id
        {
            let parent = repository
                .status_including_deleted(parent_id)
                .await
                .map_err(|_| HandlerFailure::retry("reply target lookup failed"))?;
            if let Some(parent) = parent {
                repository
                    .account(parent.account_id)
                    .await
                    .map_err(|_| HandlerFailure::retry("reply author lookup failed"))?
                    .map_or((String::new(), None), |parent_account| {
                        let atom_uri = if parent_account.domain.is_none() {
                            Some(parent.uri.clone().unwrap_or_else(|| {
                                format!(
                                    "tag:{},{}:objectId={}:objectType=Status",
                                    config.local_domain,
                                    parent.created_at.date(),
                                    parent.id
                                )
                            }))
                        } else {
                            parent.uri.clone()
                        };
                        (
                            activitypub::status_uri(&config.origin, &parent_account, &parent),
                            atom_uri,
                        )
                    })
            } else {
                (String::new(), None)
            }
        } else {
            (String::new(), None)
        };
        let in_reply_to_url = (!in_reply_to_url.is_empty()).then_some(in_reply_to_url);
        let conversation = match status.conversation_id {
            Some(conversation_id) => repository
                .conversation(conversation_id)
                .await
                .map_err(|_| HandlerFailure::retry("status conversation lookup failed"))?
                .and_then(|conversation| conversation.uri),
            None => None,
        };
        let (quoted_link, quoted_identifier, quote_authorization) =
            activitypub_quote_parts(&repository, config, status_id).await?;
        let emojis = repository
            .activitypub_status_emojis(status_id)
            .await
            .map_err(|_| HandlerFailure::retry("status emoji lookup failed"))?;
        let mut object = activitypub::note(
            &config.origin,
            &config.local_domain,
            &status,
            &account,
            &config.media_root_url,
            &media,
            &mentions,
            &hashtags,
            &emojis,
            quoted_link.as_deref(),
            in_reply_to_url.as_deref(),
            in_reply_to_atom_uri.as_deref(),
            conversation.as_deref(),
            quoted_identifier.as_deref(),
            quote_authorization.as_deref(),
            None,
            status_stat
                .as_ref()
                .map_or(0, |stats| stats.favourites_count),
            status_stat.as_ref().map_or(0, |stats| stats.reblogs_count),
        );
        if activity_type == "Update" {
            let object_uri = object["id"]
                .as_str()
                .ok_or_else(|| HandlerFailure::permanent("status Note has no ID"))?
                .to_owned();
            let edited_at = requested_edited_at
                .or(status.edited_at)
                .unwrap_or(status.updated_at);
            object["updated"] = json!(activitypub::timestamp(edited_at));
            let update_uri = format!("{object_uri}#updates/{}", edited_at.and_utc().timestamp());
            activitypub::update_with_uris(
                &update_uri,
                &activitypub::actor_url(&config.origin, &account),
                edited_at,
                object,
            )
        } else {
            activitypub::create(&config.origin, &account, &status, object)
        }
    };
    let include_unsafe_reach = is_delete;
    let follower_ids = if matches!(
        status.visibility,
        StatusVisibility::Public | StatusVisibility::Unlisted | StatusVisibility::Private
    ) {
        repository
            .activitypub_remote_follower_ids(status.account_id, include_unsafe_reach)
            .await
            .map_err(|_| HandlerFailure::retry("remote follower lookup failed"))?
    } else {
        Vec::new()
    };
    let mut recipient_ids = follower_ids.into_iter().collect::<BTreeSet<_>>();
    recipient_ids.extend(explicit_recipient_ids.iter().copied());
    let reached_account_ids = if status.reblog_of_id.is_some() {
        repository
            .activitypub_reblog_target_account_ids(status_id, include_unsafe_reach)
            .await
            .map_err(|_| HandlerFailure::retry("reblog target reach lookup failed"))?
    } else {
        repository
            .activitypub_status_reach_account_ids(status_id, include_unsafe_reach)
            .await
            .map_err(|_| HandlerFailure::retry("status reach lookup failed"))?
    };
    recipient_ids.extend(reached_account_ids);
    recipient_ids.extend(mentioned_recipient_ids);
    let mut inboxes = BTreeMap::new();
    for recipient_id in recipient_ids {
        let Some(follower) = repository
            .account(recipient_id)
            .await
            .map_err(|_| HandlerFailure::retry("remote follower account lookup failed"))?
        else {
            continue;
        };
        let Some(domain) = follower.domain.as_deref() else {
            continue;
        };
        if follower.protocol.0 != 1 {
            continue;
        }
        if follower.suspended_at.is_some() && !include_unsafe_reach {
            continue;
        }
        if !repository
            .remote_domain_allowed(domain, config.limited_federation)
            .await
            .map_err(|_| HandlerFailure::retry("remote domain policy lookup failed"))?
        {
            continue;
        }
        let inbox_url = if follower.shared_inbox_url.is_empty() {
            follower.inbox_url
        } else {
            follower.shared_inbox_url
        };
        if !inbox_url.is_empty() {
            inboxes
                .entry(inbox_url)
                .or_insert_with(|| domain.to_owned());
        }
    }
    if status.visibility == StatusVisibility::Public {
        let relay_inboxes = repository
            .activitypub_relay_inboxes()
            .await
            .map_err(|_| HandlerFailure::retry("status relay lookup failed"))?;
        for inbox_url in relay_inboxes {
            let Ok(parsed) = Url::parse(&inbox_url) else {
                continue;
            };
            let Some(host) = parsed.host_str() else {
                continue;
            };
            let authority = parsed
                .port()
                .map_or_else(|| host.to_owned(), |port| format!("{host}:{port}"));
            let Ok(domain) = canonical_remote_domain(&authority) else {
                continue;
            };
            if repository
                .remote_domain_allowed(&domain, config.limited_federation)
                .await
                .map_err(|_| HandlerFailure::retry("status relay policy lookup failed"))?
            {
                inboxes.insert(inbox_url, domain);
            }
        }
    }
    if inboxes.is_empty() {
        return Ok(());
    }

    let mut transaction = pool
        .begin()
        .await
        .map_err(|_| HandlerFailure::retry("delivery outbox transaction failed"))?;
    let activity_id = activity["id"]
        .as_str()
        .ok_or_else(|| HandlerFailure::permanent("status activity has no ID"))?;
    for (inbox_url, remote_domain) in inboxes {
        let logical_key = match activity_type {
            "Create" => delivery_logical_key(status_id, &inbox_url),
            "Update" => update_delivery_logical_key(
                status_id,
                activity_id,
                update_version_micros.expect("Update activity has an edit version"),
                &inbox_url,
            ),
            "Delete" => delete_delivery_logical_key(status_id, &inbox_url),
            _ => unreachable!("activity type was validated above"),
        };
        let delivery = JobSpec::new(
            Lane::Push,
            ACTIVITYPUB_DELIVERY_JOB_KIND,
            json!({
                "status_id": status_id,
                "source_account_id": account.id,
                "inbox_url": inbox_url,
                "body": activity.clone(),
                "activity_type": activity_type,
                "edited_at_micros": update_version_micros,
                "remote_domain": remote_domain
            }),
        )
        .logical_key(logical_key);
        record_outbox_once_in(&mut transaction, &delivery)
            .await
            .map_err(|_| HandlerFailure::retry("delivery outbox write failed"))?;
    }
    transaction
        .commit()
        .await
        .map_err(|_| HandlerFailure::retry("delivery outbox commit failed"))?;
    Ok(())
}

#[allow(clippy::too_many_lines)]
async fn distribute_account_update(
    pool: PgPool,
    config: &ActivityPubDeliveryConfig,
    account_id: i64,
    updated_at_micros: i64,
) -> Result<(), HandlerFailure> {
    let requested_updated_at = DateTime::<Utc>::from_timestamp_micros(updated_at_micros)
        .map(|timestamp| timestamp.naive_utc())
        .ok_or_else(|| HandlerFailure::permanent("account update timestamp is invalid"))?;
    let repository = Repository::from_pool(pool.clone());
    let Some(account) = repository
        .account(account_id)
        .await
        .map_err(|_| HandlerFailure::retry("account update lookup failed"))?
    else {
        return Ok(());
    };
    if account.domain.is_some() || account.updated_at != requested_updated_at {
        return Ok(());
    }
    let hashtags = repository
        .activitypub_account_hashtags(account_id)
        .await
        .map_err(|_| HandlerFailure::retry("account update hashtag lookup failed"))?;
    let emojis = repository
        .activitypub_account_emojis(account_id)
        .await
        .map_err(|_| HandlerFailure::retry("account update emoji lookup failed"))?;
    let activity = activitypub::update_actor(
        &config.origin,
        &config.local_domain,
        &config.media_root_url,
        &account,
        &hashtags,
        &emojis,
    );
    let recipient_ids = repository
        .activitypub_account_reach_account_ids(account_id)
        .await
        .map_err(|_| HandlerFailure::retry("account update reach lookup failed"))?;
    let relay_inboxes = repository
        .activitypub_relay_inboxes()
        .await
        .map_err(|_| HandlerFailure::retry("account update relay lookup failed"))?;
    let mut inboxes = BTreeMap::new();
    for recipient_id in recipient_ids {
        let Some(recipient) = repository
            .account(recipient_id)
            .await
            .map_err(|_| HandlerFailure::retry("account update recipient lookup failed"))?
        else {
            continue;
        };
        let Some(domain) = recipient.domain.as_deref() else {
            continue;
        };
        if recipient.protocol.0 != 1 {
            continue;
        }
        if !repository
            .remote_domain_allowed(domain, config.limited_federation)
            .await
            .map_err(|_| HandlerFailure::retry("account update domain policy lookup failed"))?
        {
            continue;
        }
        let inbox_url = if recipient.shared_inbox_url.is_empty() {
            recipient.inbox_url
        } else {
            recipient.shared_inbox_url
        };
        if !inbox_url.is_empty() {
            inboxes
                .entry(inbox_url)
                .or_insert_with(|| domain.to_owned());
        }
    }
    for inbox_url in relay_inboxes {
        let Ok(parsed) = Url::parse(&inbox_url) else {
            continue;
        };
        let Some(host) = parsed.host_str() else {
            continue;
        };
        let authority = parsed
            .port()
            .map_or_else(|| host.to_owned(), |port| format!("{host}:{port}"));
        let Ok(domain) = canonical_remote_domain(&authority) else {
            continue;
        };
        if repository
            .remote_domain_allowed(&domain, config.limited_federation)
            .await
            .map_err(|_| HandlerFailure::retry("account update relay policy lookup failed"))?
        {
            inboxes.insert(inbox_url, domain);
        }
    }
    if inboxes.is_empty() {
        return Ok(());
    }

    let activity_id = activity["id"]
        .as_str()
        .ok_or_else(|| HandlerFailure::permanent("account update has no ID"))?;
    let mut transaction = pool
        .begin()
        .await
        .map_err(|_| HandlerFailure::retry("account update outbox transaction failed"))?;
    for (inbox_url, remote_domain) in inboxes {
        let delivery = JobSpec::new(
            Lane::Push,
            ACTIVITYPUB_DELIVERY_JOB_KIND,
            json!({
                "source_account_id": account_id,
                "inbox_url": inbox_url,
                "remote_domain": remote_domain,
                "body": activity,
                "activity_type": "AccountUpdate",
                "updated_at_micros": updated_at_micros
            }),
        )
        .logical_key(account_update_delivery_logical_key(
            account_id,
            activity_id,
            updated_at_micros,
            &inbox_url,
        ));
        record_outbox_once_in(&mut transaction, &delivery)
            .await
            .map_err(|_| HandlerFailure::retry("account update delivery outbox write failed"))?;
    }
    transaction
        .commit()
        .await
        .map_err(|_| HandlerFailure::retry("account update delivery outbox commit failed"))?;
    Ok(())
}

async fn distribute_account_delete(
    pool: PgPool,
    config: &ActivityPubDeliveryConfig,
    account_id: i64,
    actor_uri: &str,
) -> Result<(), HandlerFailure> {
    let repository = Repository::from_pool(pool.clone());
    let Some(account) = repository
        .account(account_id)
        .await
        .map_err(|_| HandlerFailure::retry("account deletion lookup failed"))?
    else {
        return Ok(());
    };
    if account.domain.is_some() || account.suspended_at.is_none() {
        return Ok(());
    }

    let mut inboxes = BTreeMap::new();
    let remote_inboxes = repository
        .activitypub_remote_inboxes()
        .await
        .map_err(|_| HandlerFailure::retry("account deletion recipient lookup failed"))?;
    for (inbox_url, domain) in remote_inboxes {
        if repository
            .remote_domain_allowed(&domain, config.limited_federation)
            .await
            .map_err(|_| HandlerFailure::retry("account deletion domain policy lookup failed"))?
            && !inbox_url.is_empty()
        {
            inboxes.entry(inbox_url).or_insert(domain);
        }
    }
    let relay_inboxes = repository
        .activitypub_relay_inboxes()
        .await
        .map_err(|_| HandlerFailure::retry("account deletion relay lookup failed"))?;
    for inbox_url in relay_inboxes {
        let Ok(parsed) = Url::parse(&inbox_url) else {
            continue;
        };
        let Some(host) = parsed.host_str() else {
            continue;
        };
        let authority = parsed
            .port()
            .map_or_else(|| host.to_owned(), |port| format!("{host}:{port}"));
        let Ok(domain) = canonical_remote_domain(&authority) else {
            continue;
        };
        if repository
            .remote_domain_allowed(&domain, config.limited_federation)
            .await
            .map_err(|_| HandlerFailure::retry("account deletion relay policy lookup failed"))?
        {
            inboxes.entry(inbox_url).or_insert(domain);
        }
    }
    if inboxes.is_empty() {
        return Ok(());
    }

    let activity = activitypub::delete_actor_with_uris(&format!("{actor_uri}#delete"), actor_uri);
    let mut transaction = pool
        .begin()
        .await
        .map_err(|_| HandlerFailure::retry("account deletion outbox transaction failed"))?;
    if !account_delete_delivery_is_current(&mut transaction, account_id)
        .await
        .map_err(|_| HandlerFailure::retry("account deletion state lookup failed"))?
    {
        return Ok(());
    }
    for (inbox_url, remote_domain) in inboxes {
        let delivery = JobSpec::new(
            Lane::Push,
            ACTIVITYPUB_DELIVERY_JOB_KIND,
            json!({
                "source_account_id": account_id,
                "inbox_url": inbox_url,
                "remote_domain": remote_domain,
                "body": activity,
                "activity_type": "AccountDelete"
            }),
        )
        .logical_key(activitypub::delete_actor_delivery_logical_key(
            actor_uri, &inbox_url,
        ));
        record_outbox_once_in(&mut transaction, &delivery)
            .await
            .map_err(|_| HandlerFailure::retry("account deletion delivery outbox write failed"))?;
    }
    transaction
        .commit()
        .await
        .map_err(|_| HandlerFailure::retry("account deletion delivery outbox commit failed"))?;
    Ok(())
}

async fn account_delete_delivery_is_current(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
) -> Result<bool, WriteError> {
    let Some((domain, suspended_at)) =
        sqlx::query_as::<_, (Option<String>, Option<NaiveDateTime>)>(
            "SELECT domain, suspended_at FROM accounts WHERE id = $1 FOR UPDATE",
        )
        .bind(account_id)
        .fetch_optional(&mut **transaction)
        .await?
    else {
        return Ok(false);
    };
    if domain.is_some() || suspended_at.is_none() {
        return Ok(false);
    }
    Ok(true)
}

async fn process_account_purge_job(
    runtime_queue: Queue,
    pool: PgPool,
    job: &ClaimedJob,
    media_root: Option<PaperclipRoot>,
) -> Result<(), HandlerFailure> {
    let account_id = job
        .arguments
        .get("account_id")
        .and_then(Value::as_i64)
        .ok_or_else(|| HandlerFailure::permanent("account purge job is missing its account ID"))?;
    let origin = job.arguments.get("origin").and_then(Value::as_str);
    let expected_deletion_request_id = account_purge_deletion_request_id(&job.arguments)?;
    let expected_deletion_created_at = account_purge_deletion_created_at(&job.arguments)?;
    let existing_paths = account_purge_cleanup_paths(&job.arguments)?;
    let writer = WriteRepository::from_pool(pool);
    let result = writer
        .with_account_lock(account_id, || async {
            let cleanup_paths = if let Some(paths) = existing_paths {
                paths
            } else {
                let media = writer
                    .account_purge_media_metadata(
                        account_id,
                        expected_deletion_request_id,
                        expected_deletion_created_at,
                    )
                    .await?;
                let paths = paperclip_cleanup_paths(&media);
                let merged = runtime_queue
                    .merge_job_arguments(job, &json!({"cleanup_paths": paths.clone()}))
                    .await
                    .map_err(WriteError::Job)?;
                if !merged {
                    return Err(WriteError::Conflict);
                }
                paths
            };
            if !cleanup_paths.is_empty() && media_root.is_none() {
                return Err(WriteError::Validation(
                    "account purge requires a configured media root",
                ));
            }
            let outcome = writer
                .purge_account_after_deletion(
                    account_id,
                    expected_deletion_request_id,
                    expected_deletion_created_at,
                    origin,
                )
                .await?;
            if matches!(
                outcome,
                AccountPurgeOutcome::Purged | AccountPurgeOutcome::AlreadyPurged
            ) && let Some(media_root) = media_root.as_ref()
            {
                remove_paperclip_paths_io(media_root, &cleanup_paths)
                    .map_err(WriteError::Filesystem)?;
            }
            Ok(())
        })
        .await;
    result.map_err(|error| match error {
        WriteError::Conflict => HandlerFailure::retry("account purge lease was lost"),
        WriteError::Filesystem(_) => HandlerFailure::retry("account purge media removal failed"),
        WriteError::Validation(message)
            if message == "account purge requires a configured media root" =>
        {
            HandlerFailure::retry(message)
        }
        WriteError::InvalidInput(_) => HandlerFailure::permanent("account purge job is invalid"),
        _ => HandlerFailure::retry("account purge failed"),
    })
}

async fn process_domain_block_job(
    pool: PgPool,
    arguments: &Value,
    media_root: Option<PaperclipRoot>,
) -> Result<(), HandlerFailure> {
    let domain_block_id = arguments
        .get("domain_block_id")
        .and_then(Value::as_i64)
        .ok_or_else(|| HandlerFailure::permanent("domain block job is missing its block ID"))?;
    let severance_event_id = arguments.get("severance_event_id").and_then(Value::as_i64);
    let origin = arguments
        .get("origin")
        .and_then(Value::as_str)
        .ok_or_else(|| HandlerFailure::permanent("domain block job is missing its origin"))?;
    let writer = WriteRepository::from_pool(pool);
    let domain = writer
        .domain_block_domain(domain_block_id)
        .await
        .map_err(|_| HandlerFailure::retry("domain block lookup failed"))?;
    let result = if let Some(domain) = domain {
        writer
            .with_domain_lock(&domain, || async {
                let media = writer.domain_block_media_metadata(domain_block_id).await?;
                if let Some(media_root) = media_root.as_ref() {
                    remove_paperclip_files_io(media_root, &media)?;
                }
                writer
                    .process_domain_block_job_locked(domain_block_id, severance_event_id, origin)
                    .await
            })
            .await
    } else {
        writer
            .process_domain_block_job(domain_block_id, severance_event_id, origin)
            .await
    };
    result.map_err(|error| match error {
        WriteError::InvalidInput(_) | WriteError::Validation(_) => {
            HandlerFailure::permanent("domain block job arguments are invalid")
        }
        _ => HandlerFailure::retry("domain block cleanup failed"),
    })
}

async fn process_domain_purge_job(
    pool: PgPool,
    arguments: &Value,
    media_root: Option<PaperclipRoot>,
) -> Result<(), HandlerFailure> {
    let domain = arguments
        .get("domain")
        .and_then(Value::as_str)
        .ok_or_else(|| HandlerFailure::permanent("domain purge job is missing its domain"))?;
    let writer = WriteRepository::from_pool(pool);
    let result = writer
        .with_domain_lock(domain, || async {
            let media = writer.domain_purge_media_metadata(domain).await?;
            if let Some(media_root) = media_root.as_ref() {
                remove_paperclip_files_io(media_root, &media)?;
            }
            writer.process_domain_purge_job(domain).await
        })
        .await;
    result.map_err(|error| match error {
        WriteError::InvalidInput(_) | WriteError::Validation(_) => {
            HandlerFailure::permanent("domain purge job domain is invalid")
        }
        _ => HandlerFailure::retry("domain purge failed"),
    })
}

fn remove_paperclip_files(
    root: &PaperclipRoot,
    metadata: &[PaperclipMetadata],
) -> Result<(), HandlerFailure> {
    remove_paperclip_files_io(root, metadata)
        .map_err(|_| HandlerFailure::retry("domain block media removal failed"))
}

fn remove_paperclip_files_io(
    root: &PaperclipRoot,
    metadata: &[PaperclipMetadata],
) -> io::Result<()> {
    let paths = paperclip_cleanup_paths(metadata);
    remove_paperclip_paths_io(root, &paths)
}

fn remove_paperclip_paths_io(root: &PaperclipRoot, paths: &[String]) -> io::Result<()> {
    for path in paths {
        root.remove_file(Path::new(path))?;
    }
    Ok(())
}

fn paperclip_cleanup_paths(metadata: &[PaperclipMetadata]) -> Vec<String> {
    let mut paths = BTreeSet::new();
    for metadata in metadata {
        for style in ["original", "small", "static"] {
            if let Some(path) = metadata.relative_path(style) {
                paths.insert(path);
            }
        }
    }
    paths.into_iter().collect()
}

fn account_purge_cleanup_paths(arguments: &Value) -> Result<Option<Vec<String>>, HandlerFailure> {
    let Some(value) = arguments.get("cleanup_paths") else {
        return Ok(None);
    };
    let paths = value.as_array().ok_or_else(|| {
        HandlerFailure::permanent("account purge cleanup paths must be a JSON array")
    })?;
    let mut validated = BTreeSet::new();
    for path in paths {
        let path = path.as_str().ok_or_else(|| {
            HandlerFailure::permanent("account purge cleanup paths must be strings")
        })?;
        if !safe_cleanup_path(path) {
            return Err(HandlerFailure::permanent(
                "account purge cleanup path is invalid",
            ));
        }
        validated.insert(path.to_owned());
    }
    Ok(Some(validated.into_iter().collect()))
}

fn account_purge_deletion_created_at(
    arguments: &Value,
) -> Result<Option<NaiveDateTime>, HandlerFailure> {
    let Some(value) = arguments.get("deletion_created_at_micros") else {
        return Ok(None);
    };
    let micros = value.as_i64().ok_or_else(|| {
        HandlerFailure::permanent("account purge deletion timestamp must be an integer")
    })?;
    DateTime::<Utc>::from_timestamp_micros(micros)
        .map(|timestamp| Some(timestamp.naive_utc()))
        .ok_or_else(|| HandlerFailure::permanent("account purge deletion timestamp is invalid"))
}

fn account_purge_deletion_request_id(arguments: &Value) -> Result<Option<i64>, HandlerFailure> {
    let Some(value) = arguments.get("deletion_request_id") else {
        return Ok(None);
    };
    let request_id = value
        .as_i64()
        .ok_or_else(|| HandlerFailure::permanent("account purge request ID must be an integer"))?;
    (request_id > 0)
        .then_some(Some(request_id))
        .ok_or_else(|| HandlerFailure::permanent("account purge request ID is invalid"))
}

fn safe_cleanup_path(path: &str) -> bool {
    if path.is_empty() {
        return false;
    }
    let mut has_component = false;
    for component in Path::new(path).components() {
        let Component::Normal(component) = component else {
            return false;
        };
        let Some(component) = component.to_str() else {
            return false;
        };
        if component.is_empty()
            || matches!(component, "." | "..")
            || component.contains(['/', '\\', '\0'])
            || component.chars().any(char::is_control)
        {
            return false;
        }
        has_component = true;
    }
    has_component
}

async fn process_local_media_cleanup_job(
    pool: PgPool,
    root: PaperclipRoot,
    arguments: &Value,
) -> Result<(), HandlerFailure> {
    let account_id = arguments
        .get("account_id")
        .and_then(Value::as_i64)
        .filter(|id| *id > 0)
        .ok_or_else(|| HandlerFailure::permanent("local media cleanup account ID is invalid"))?;
    let media_id = arguments
        .get("media_id")
        .and_then(Value::as_i64)
        .filter(|id| *id > 0)
        .ok_or_else(|| HandlerFailure::permanent("local media cleanup media ID is invalid"))?;
    let action = arguments
        .get("action")
        .and_then(Value::as_str)
        .filter(|action| matches!(*action, "rollback_create" | "delete"))
        .ok_or_else(|| HandlerFailure::permanent("local media cleanup action is invalid"))?;
    let paths = arguments
        .get("paths")
        .and_then(Value::as_array)
        .ok_or_else(|| HandlerFailure::permanent("local media cleanup paths are missing"))?
        .iter()
        .map(|path| {
            path.as_str()
                .filter(|path| safe_cleanup_path(path))
                .filter(|path| {
                    parse_paperclip_path(path).is_some_and(|parsed| {
                        parsed.id() == media_id
                            && matches!(
                                parsed.attachment(),
                                PaperclipAttachment::MediaFile
                                    | PaperclipAttachment::MediaThumbnail
                            )
                    })
                })
                .map(str::to_owned)
                .ok_or_else(|| HandlerFailure::permanent("local media cleanup path is invalid"))
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    if paths.is_empty() {
        return Err(HandlerFailure::permanent(
            "local media cleanup paths are missing",
        ));
    }

    let writer = WriteRepository::from_pool(pool.clone());
    writer
        .with_account_lock(account_id, || async {
            let mut transaction = pool.begin().await?;
            let row = sqlx::query_as::<_, (Option<i64>, Option<String>)>(
                "SELECT status_id, file_file_name FROM media_attachments
                   WHERE id = $1 AND account_id = $2 FOR UPDATE",
            )
            .bind(media_id)
            .bind(account_id)
            .fetch_optional(&mut *transaction)
            .await?;
            if action == "rollback_create"
                && row
                    .as_ref()
                    .is_some_and(|(_, file_name)| file_name.is_some())
            {
                transaction.commit().await?;
                return Ok(());
            }
            if row
                .as_ref()
                .is_some_and(|(status_id, _)| status_id.is_some())
            {
                return Err(WriteError::Validation(
                    "local media cleanup target is attached",
                ));
            }
            for path in &paths {
                root.remove_file(Path::new(path))?;
            }
            sqlx::query(
                "DELETE FROM media_attachments
                   WHERE id = $1 AND account_id = $2 AND status_id IS NULL",
            )
            .bind(media_id)
            .bind(account_id)
            .execute(&mut *transaction)
            .await?;
            transaction.commit().await?;
            Ok(())
        })
        .await
        .map_err(|error| match error {
            WriteError::Validation(_) | WriteError::InvalidInput(_) => {
                HandlerFailure::permanent("local media cleanup target is invalid")
            }
            WriteError::Filesystem(_) => HandlerFailure::retry("local media unlink failed"),
            _ => HandlerFailure::retry("local media cleanup failed"),
        })
}

async fn process_activitypub_emoji_cleanup_job(
    pool: PgPool,
    root: PaperclipRoot,
    arguments: &Value,
) -> Result<(), HandlerFailure> {
    let emoji_id = arguments
        .get("emoji_id")
        .and_then(Value::as_i64)
        .filter(|id| *id > 0)
        .ok_or_else(|| HandlerFailure::permanent("emoji cleanup ID is invalid"))?;
    let paths = arguments
        .get("paths")
        .and_then(Value::as_array)
        .ok_or_else(|| HandlerFailure::permanent("emoji cleanup paths are missing"))?
        .iter()
        .map(|path| {
            path.as_str()
                .filter(|path| safe_cleanup_path(path))
                .filter(|path| {
                    parse_paperclip_path(path).is_some_and(|parsed| {
                        parsed.id() == emoji_id
                            && parsed.attachment() == PaperclipAttachment::CustomEmojiImage
                    })
                })
                .map(str::to_owned)
                .ok_or_else(|| HandlerFailure::permanent("emoji cleanup path is invalid"))
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    if paths.is_empty() {
        return Err(HandlerFailure::permanent("emoji cleanup paths are missing"));
    }

    let mut transaction = pool
        .begin()
        .await
        .map_err(|_| HandlerFailure::retry("emoji cleanup transaction failed"))?;
    let current =
        sqlx::query_as::<_, (Option<String>, Option<String>, Option<i32>, Option<String>)>(
            "SELECT image_file_name, image_content_type, image_storage_schema_version, domain
           FROM custom_emojis WHERE id = $1 FOR UPDATE",
        )
        .bind(emoji_id)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|_| HandlerFailure::retry("emoji cleanup lookup failed"))?;
    let current_paths = current
        .and_then(
            |(file_name, content_type, storage_schema_version, domain)| {
                file_name.map(|file_name| PaperclipMetadata {
                    attachment: PaperclipAttachment::CustomEmojiImage,
                    id: emoji_id,
                    remote: domain.is_some(),
                    storage_schema_version,
                    file_name,
                    content_type,
                    variant: None,
                })
            },
        )
        .into_iter()
        .flat_map(|metadata| {
            ["original", "static"]
                .into_iter()
                .filter_map(move |style| metadata.relative_path(style))
        })
        .collect::<BTreeSet<_>>();
    for path in paths.difference(&current_paths) {
        root.remove_file(Path::new(path))
            .map_err(|_| HandlerFailure::retry("emoji cleanup unlink failed"))?;
    }
    transaction
        .commit()
        .await
        .map_err(|_| HandlerFailure::retry("emoji cleanup commit failed"))?;
    Ok(())
}

async fn process_notification_job(pool: PgPool, arguments: &Value) -> Result<(), HandlerFailure> {
    let recipient_account_id = arguments
        .get("recipient_account_id")
        .and_then(Value::as_i64)
        .ok_or_else(|| HandlerFailure::permanent("notification job is missing its recipient"))?;
    let activity_id = arguments
        .get("activity_id")
        .and_then(Value::as_i64)
        .ok_or_else(|| HandlerFailure::permanent("notification job is missing its activity"))?;
    let activity = match arguments.get("activity_type").and_then(Value::as_str) {
        Some("mention") => NotificationActivity::Mention { id: activity_id },
        Some("favourite") => NotificationActivity::Favourite { id: activity_id },
        Some("reblog") => NotificationActivity::Reblog { id: activity_id },
        Some("follow") => NotificationActivity::Follow { id: activity_id },
        Some("follow_request") => NotificationActivity::FollowRequest { id: activity_id },
        Some("update") => NotificationActivity::Update { id: activity_id },
        Some("quoted_update") => NotificationActivity::QuotedUpdate { id: activity_id },
        Some("admin.report") => NotificationActivity::AdminReport { id: activity_id },
        Some("AccountWarning") => NotificationActivity::ModerationWarning { id: activity_id },
        _ => {
            return Err(HandlerFailure::permanent(
                "notification job has an unsupported activity",
            ));
        }
    };
    let silenced = arguments
        .get("silenced")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    WriteRepository::from_pool(pool)
        .create_notification(NotificationCreate {
            recipient_account_id,
            activity,
            silenced,
        })
        .await
        .map(|_| ())
        .map_err(|_| HandlerFailure::retry("notification creation failed"))
}

#[allow(clippy::too_many_lines)]
async fn process_notification_unfilter_job(
    pool: PgPool,
    arguments: &Value,
) -> Result<(), HandlerFailure> {
    let account_id = arguments
        .get("account_id")
        .and_then(Value::as_i64)
        .ok_or_else(|| HandlerFailure::permanent("unfilter job is missing its account"))?;
    let from_account_id = arguments
        .get("from_account_id")
        .and_then(Value::as_i64)
        .ok_or_else(|| HandlerFailure::permanent("unfilter job is missing its sender"))?;
    let mut transaction = pool
        .begin()
        .await
        .map_err(|_| HandlerFailure::retry("notification unfilter transaction failed"))?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(account_id)
        .execute(&mut *transaction)
        .await
        .map_err(|_| HandlerFailure::retry("notification unfilter lock failed"))?;
    let conversation_rows = sqlx::query_as::<_, (i64, i32)>(
        "WITH direct AS ( \
           SELECT DISTINCT notification.account_id, status.conversation_id, status.id AS status_id, \
                  status.account_id AS author_id, \
                  ARRAY( \
                    SELECT DISTINCT participant.account_id \
                      FROM ( \
                        SELECT status_mention.account_id \
                          FROM mentions status_mention \
                         WHERE status_mention.status_id = status.id \
                           AND status_mention.silent = false \
                        UNION ALL \
                        SELECT status.account_id \
                      ) participant \
                     WHERE participant.account_id <> notification.account_id \
                     ORDER BY participant.account_id \
                  ) AS participant_account_ids \
             FROM notifications notification \
             JOIN mentions mention ON mention.id = notification.activity_id \
             JOIN statuses status ON status.id = mention.status_id \
            WHERE notification.account_id = $1 \
              AND notification.from_account_id = $2 \
              AND notification.activity_type = 'Mention' \
              AND notification.type = 'mention' \
              AND notification.filtered = true \
              AND status.visibility = 3 \
              AND status.deleted_at IS NULL \
              AND status.conversation_id IS NOT NULL \
         ), grouped AS ( \
           SELECT account_id, conversation_id, participant_account_ids, \
                  max(status_id) AS last_status_id, \
                  array_agg(DISTINCT status_id ORDER BY status_id) AS status_ids, \
                  bool_or(author_id <> account_id) AS unread \
             FROM direct \
            GROUP BY account_id, conversation_id, participant_account_ids \
         ) \
         INSERT INTO account_conversations ( \
           account_id, conversation_id, last_status_id, participant_account_ids, status_ids, unread) \
         SELECT account_id, conversation_id, last_status_id, participant_account_ids, status_ids, unread \
           FROM grouped \
         ON CONFLICT (account_id, conversation_id, participant_account_ids) \
         DO UPDATE SET \
           last_status_id = ( \
             SELECT max(status_id) \
               FROM unnest(account_conversations.status_ids || EXCLUDED.status_ids) ids(status_id) \
           ), \
           status_ids = ( \
             SELECT ARRAY( \
               SELECT DISTINCT status_id \
                 FROM unnest(account_conversations.status_ids || EXCLUDED.status_ids) ids(status_id) \
                ORDER BY status_id \
             ) \
           ), \
           unread = EXCLUDED.unread, \
           lock_version = account_conversations.lock_version + 1 \
          WHERE NOT (EXCLUDED.status_ids <@ account_conversations.status_ids) \
          RETURNING account_conversations.id, account_conversations.lock_version",
    )
    .bind(account_id)
    .bind(from_account_id)
    .fetch_all(&mut *transaction)
    .await
    .map_err(|_| HandlerFailure::retry("notification conversation repair failed"))?;
    for (conversation_id, lock_version) in conversation_rows {
        record_conversation_stream_event(
            &mut transaction,
            account_id,
            conversation_id,
            lock_version,
        )
        .await?;
    }
    let unfiltered_notification_ids = sqlx::query_scalar::<_, i64>(
        "UPDATE notifications SET filtered = false \
          WHERE account_id = $1 AND from_account_id = $2 AND filtered = true \
          RETURNING id",
    )
    .bind(account_id)
    .bind(from_account_id)
    .fetch_all(&mut *transaction)
    .await
    .map_err(|_| HandlerFailure::retry("notification unfilter failed"))?;
    if let Some(notification_id) = unfiltered_notification_ids.iter().max().copied() {
        record_stream_event_in(
            &mut transaction,
            account_id,
            "notifications_merged",
            notification_id,
            &event_logical_key(
                account_id,
                "notifications_merged",
                from_account_id,
                notification_id,
            ),
        )
        .await
        .map_err(|_| HandlerFailure::retry("notification merge stream write failed"))?;
    }
    transaction
        .commit()
        .await
        .map_err(|_| HandlerFailure::retry("notification unfilter commit failed"))?;
    Ok(())
}

async fn process_notification_cleanup_job(
    pool: PgPool,
    arguments: &Value,
) -> Result<(), HandlerFailure> {
    let account_id = arguments
        .get("account_id")
        .and_then(Value::as_i64)
        .ok_or_else(|| {
            HandlerFailure::permanent("notification cleanup job is missing its account")
        })?;
    let from_account_id = arguments
        .get("from_account_id")
        .and_then(Value::as_i64)
        .ok_or_else(|| {
            HandlerFailure::permanent("notification cleanup job is missing its sender")
        })?;
    loop {
        let deleted = sqlx::query(
            "DELETE FROM notifications
              WHERE id IN (
                SELECT id FROM notifications
                 WHERE account_id = $1 AND from_account_id = $2 AND filtered = true
                 ORDER BY id DESC LIMIT $3
              )",
        )
        .bind(account_id)
        .bind(from_account_id)
        .bind(NOTIFICATION_CLEANUP_BATCH_SIZE)
        .execute(&pool)
        .await
        .map_err(|_| HandlerFailure::retry("filtered notification cleanup failed"))?;
        if deleted.rows_affected() < NOTIFICATION_CLEANUP_BATCH_SIZE as u64 {
            break;
        }
    }
    Ok(())
}

async fn record_conversation_stream_event(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    account_id: i64,
    conversation_id: i64,
    lock_version: i32,
) -> Result<(), HandlerFailure> {
    record_stream_event_in(
        transaction,
        account_id,
        "conversation",
        conversation_id,
        &event_logical_key(
            account_id,
            "conversation",
            conversation_id,
            i64::from(lock_version),
        ),
    )
    .await
    .map(|_| ())
    .map_err(|_| HandlerFailure::retry("notification conversation stream write failed"))
}

fn account_update_delivery_is_current(
    activity_id: Option<&str>,
    actor_uri: &str,
    updated_at: NaiveDateTime,
    requested_updated_at_micros: Option<i64>,
) -> bool {
    if requested_updated_at_micros
        .is_some_and(|requested| requested != updated_at.and_utc().timestamp_micros())
    {
        return false;
    }
    let expected_activity_id = format!("{actor_uri}#updates/{}", updated_at.and_utc().timestamp());
    activity_id == Some(expected_activity_id.as_str())
}

#[allow(clippy::too_many_lines)]
async fn deliver_activity(
    pool: PgPool,
    operational_pool: PgPool,
    config: &ActivityPubDeliveryConfig,
    fetcher: &RemoteFetcher,
    arguments: &Value,
) -> Result<(), HandlerFailure> {
    let status_id = arguments.get("status_id").and_then(Value::as_i64);
    let source_account_id = arguments
        .get("source_account_id")
        .and_then(Value::as_i64)
        .ok_or_else(|| HandlerFailure::permanent("delivery job is missing its source account"))?;
    let inbox_url = arguments
        .get("inbox_url")
        .and_then(Value::as_str)
        .ok_or_else(|| HandlerFailure::permanent("delivery job is missing its inbox URL"))?;
    let body_value = arguments
        .get("body")
        .ok_or_else(|| HandlerFailure::permanent("delivery job is missing its activity"))?;
    if !body_value.is_object() {
        return Err(HandlerFailure::permanent(
            "delivery job activity must be a JSON object",
        ));
    }
    let body = serde_json::to_vec(body_value)
        .map_err(|_| HandlerFailure::permanent("delivery activity could not be serialized"))?;
    let repository = Repository::from_pool(pool.clone());
    let delivery_edited_at_micros = arguments.get("edited_at_micros").and_then(Value::as_i64);
    let delivery_updated_at_micros = arguments.get("updated_at_micros").and_then(Value::as_i64);
    let configured_remote_domain = arguments
        .get("remote_domain")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let is_delete = body_value.get("type").and_then(Value::as_str) == Some("Delete");
    let is_undo_announce = body_value.get("type").and_then(Value::as_str) == Some("Undo")
        && body_value
            .get("object")
            .and_then(|object| object.get("type"))
            .and_then(Value::as_str)
            == Some("Announce");
    let current_status = match status_id {
        Some(status_id) if !is_delete && !is_undo_announce => {
            let status = repository
                .status(status_id)
                .await
                .map_err(|_| HandlerFailure::retry("delivery status lookup failed"))?;
            if status.is_none() {
                return Ok(());
            }
            status
        }
        _ => None,
    };
    let source_account = repository
        .account(source_account_id)
        .await
        .map_err(|_| HandlerFailure::retry("delivery source lookup failed"))?
        .ok_or_else(|| HandlerFailure::permanent("delivery source account is missing"))?;
    if source_account.domain.is_some() {
        return Err(HandlerFailure::permanent(
            "delivery source account is not local",
        ));
    }
    let source_account_permanently_unavailable = if source_account.suspended_at.is_some() {
        !repository
            .account_has_deletion_request(source_account_id)
            .await
            .map_err(|_| HandlerFailure::retry("delivery source lifecycle lookup failed"))?
    } else {
        false
    };
    if is_delete && status_id.is_none() && source_account.suspended_at.is_none() {
        return Ok(());
    }
    if body_value.get("type").and_then(Value::as_str) == Some("Update") && status_id.is_none() {
        let actor_uri = activitypub::actor_url(&config.origin, &source_account);
        if !account_update_delivery_is_current(
            body_value.get("id").and_then(Value::as_str),
            &actor_uri,
            source_account.updated_at,
            delivery_updated_at_micros,
        ) {
            return Ok(());
        }
    }
    if body_value.get("type").and_then(Value::as_str) == Some("Update")
        && let Some(status) = current_status.as_ref()
    {
        let object_uri = activitypub::status_uri(&config.origin, &source_account, status);
        let edited_at = status.edited_at.unwrap_or(status.updated_at);
        if delivery_edited_at_micros
            .is_some_and(|requested| requested != edited_at.and_utc().timestamp_micros())
        {
            return Ok(());
        }
        let expected_activity_id =
            format!("{object_uri}#updates/{}", edited_at.and_utc().timestamp());
        if body_value["id"].as_str() != Some(expected_activity_id.as_str()) {
            return Ok(());
        }
    }
    let private_key = source_account
        .private_key
        .as_ref()
        .filter(|key| key.is_present())
        .ok_or_else(|| HandlerFailure::permanent("delivery source has no private key"))?;
    let key_id = format!(
        "{}#main-key",
        activitypub::actor_url(&config.origin, &source_account)
    );
    let signer = HttpSignatureSigner {
        key_id: &key_id,
        private_key_pem: private_key.as_str(),
    };
    let inbox_url = Url::parse(inbox_url)
        .map_err(|_| HandlerFailure::permanent("delivery inbox URL is invalid"))?;
    let domain = inbox_url
        .host_str()
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| HandlerFailure::permanent("delivery inbox URL has no host"))?;
    let policy_domain = configured_remote_domain.as_deref().unwrap_or(&domain);
    if !repository
        .remote_domain_allowed(policy_domain, config.limited_federation)
        .await
        .map_err(|_| HandlerFailure::retry("delivery remote-domain policy lookup failed"))?
    {
        return Ok(());
    }
    if !relationship_delivery_is_current(&pool, body_value).await? {
        return Ok(());
    }
    if !delivery_domain_available(&operational_pool, &domain).await? {
        return Err(HandlerFailure::retry(
            "remote delivery domain is cooling down",
        ));
    }
    let delivery = if is_delete && status_id.is_none() {
        let writer = WriteRepository::from_pool(pool.clone());
        let delivery = writer
            .with_account_lock(source_account_id, || async {
                let mut transaction = writer.pool().begin().await?;
                let current =
                    account_delete_delivery_is_current(&mut transaction, source_account_id).await?;
                transaction.commit().await?;
                if !current {
                    return Ok(None);
                }
                #[cfg(feature = "test-support")]
                let delivery = if let Some(endpoint) = config.remote_delivery_endpoint {
                    fetcher
                        .post_signed_json_for_test_endpoint(
                            inbox_url.clone(),
                            &body,
                            &signer,
                            endpoint,
                        )
                        .await
                } else {
                    fetcher
                        .post_signed_json(inbox_url.clone(), &body, &signer)
                        .await
                };
                #[cfg(not(feature = "test-support"))]
                let delivery = fetcher
                    .post_signed_json(inbox_url.clone(), &body, &signer)
                    .await;
                Ok(Some(delivery))
            })
            .await
            .map_err(|_| HandlerFailure::retry("account deletion delivery lock failed"))?;
        let Some(delivery) = delivery else {
            return Ok(());
        };
        delivery
    } else {
        #[cfg(feature = "test-support")]
        if let Some(endpoint) = config.remote_delivery_endpoint {
            fetcher
                .post_signed_json_for_test_endpoint(inbox_url.clone(), &body, &signer, endpoint)
                .await
        } else {
            fetcher
                .post_signed_json(inbox_url.clone(), &body, &signer)
                .await
        }
        #[cfg(not(feature = "test-support"))]
        {
            fetcher
                .post_signed_json(inbox_url.clone(), &body, &signer)
                .await
        }
    };
    match delivery {
        Ok(_) => {
            record_domain_success(&operational_pool, &domain).await?;
            Ok(())
        }
        Err(error) => {
            let failure = delivery_failure(&error, source_account_permanently_unavailable);
            if failure.disposition == FailureDisposition::Retry
                && !matches!(error, RemoteFetchError::DomainBudgetExceeded)
            {
                record_domain_failure(&operational_pool, &domain).await?;
            }
            Err(failure)
        }
    }
}

async fn delivery_domain_available(pool: &PgPool, domain: &str) -> Result<bool, HandlerFailure> {
    sqlx::query_scalar(
        "SELECT COALESCE( \
           (SELECT retry_at <= clock_timestamp() FROM rustodon.domain_health WHERE domain = $1), \
           true)",
    )
    .bind(domain)
    .fetch_one(pool)
    .await
    .map_err(|_| HandlerFailure::retry("delivery domain health lookup failed"))
}

async fn relationship_delivery_is_current(
    pool: &PgPool,
    body: &Value,
) -> Result<bool, HandlerFailure> {
    let Some(activity_type) = body.get("type").and_then(Value::as_str) else {
        return Ok(true);
    };
    let Some(activity_uri) = body.get("id").and_then(Value::as_str) else {
        return Ok(true);
    };
    let query = match activity_type {
        "Follow" => {
            "SELECT EXISTS (
               SELECT 1 FROM follows WHERE uri = $1
               UNION ALL
               SELECT 1 FROM follow_requests WHERE uri = $1)"
        }
        "Block" => "SELECT EXISTS (SELECT 1 FROM blocks WHERE uri = $1)",
        _ => return Ok(true),
    };
    sqlx::query_scalar(query)
        .bind(activity_uri)
        .fetch_one(pool)
        .await
        .map_err(|_| HandlerFailure::retry("relationship delivery state lookup failed"))
}

async fn record_domain_success(pool: &PgPool, domain: &str) -> Result<(), HandlerFailure> {
    sqlx::query(
        "INSERT INTO rustodon.domain_health \
           (domain, failures, last_success_at, retry_at, last_error) \
         VALUES ($1, 0, clock_timestamp(), NULL, NULL) \
         ON CONFLICT (domain) DO UPDATE SET failures = 0, \
           last_success_at = clock_timestamp(), retry_at = NULL, last_error = NULL, \
           updated_at = clock_timestamp()",
    )
    .bind(domain)
    .execute(pool)
    .await
    .map(|_| ())
    .map_err(|_| HandlerFailure::retry("delivery domain health update failed"))
}

async fn record_domain_failure(pool: &PgPool, domain: &str) -> Result<(), HandlerFailure> {
    sqlx::query(
        "INSERT INTO rustodon.domain_health \
           (domain, failures, last_failure_at, retry_at, last_error) \
         VALUES ($1, 1, clock_timestamp(), clock_timestamp() + interval '15 seconds', \
                 'remote delivery failed') \
         ON CONFLICT (domain) DO UPDATE SET \
           failures = rustodon.domain_health.failures + 1, \
           last_failure_at = clock_timestamp(), \
           retry_at = clock_timestamp() + make_interval( \
             secs => LEAST(3600, 15 * (rustodon.domain_health.failures + 1))), \
           last_error = 'remote delivery failed', updated_at = clock_timestamp()",
    )
    .bind(domain)
    .execute(pool)
    .await
    .map(|_| ())
    .map_err(|_| HandlerFailure::retry("delivery domain health update failed"))
}

fn delivery_failure(
    error: &RemoteFetchError,
    source_account_permanently_unavailable: bool,
) -> HandlerFailure {
    match error {
        RemoteFetchError::UnexpectedStatus(status)
            if source_account_permanently_unavailable && *status == StatusCode::UNAUTHORIZED =>
        {
            HandlerFailure::permanent("remote delivery authorization is permanently unavailable")
        }
        RemoteFetchError::UnexpectedStatus(status)
            if *status == StatusCode::NOT_IMPLEMENTED
                || (status.is_client_error()
                    && !matches!(
                        *status,
                        StatusCode::UNAUTHORIZED
                            | StatusCode::REQUEST_TIMEOUT
                            | StatusCode::TOO_MANY_REQUESTS
                    )) =>
        {
            HandlerFailure::permanent("remote inbox rejected the activity")
        }
        RemoteFetchError::InvalidUrl
        | RemoteFetchError::Redirect
        | RemoteFetchError::TooManyRedirects
        | RemoteFetchError::UnsupportedEncoding
        | RemoteFetchError::BodyTooLarge
        | RemoteFetchError::IdentityMismatch
        | RemoteFetchError::OriginMismatch
        | RemoteFetchError::Signing => HandlerFailure::permanent("remote delivery is invalid"),
        _ => HandlerFailure::retry("remote delivery failed"),
    }
}

fn inbox_remote_failure(error: &RemoteFetchError) -> HandlerFailure {
    match error {
        RemoteFetchError::InvalidUrl
        | RemoteFetchError::Redirect
        | RemoteFetchError::TooManyRedirects
        | RemoteFetchError::MissingContentType
        | RemoteFetchError::UnsupportedContentType
        | RemoteFetchError::UnsupportedEncoding
        | RemoteFetchError::BodyTooLarge
        | RemoteFetchError::InvalidRepresentation
        | RemoteFetchError::IdentityMismatch
        | RemoteFetchError::OriginMismatch
        | RemoteFetchError::Signing
        | RemoteFetchError::BlockedAddress(_) => {
            HandlerFailure::permanent(format!("remote inbox actor is invalid: {error}"))
        }
        RemoteFetchError::UnexpectedStatus(status)
            if status.is_client_error()
                && !matches!(
                    *status,
                    StatusCode::UNAUTHORIZED
                        | StatusCode::REQUEST_TIMEOUT
                        | StatusCode::TOO_MANY_REQUESTS
                ) =>
        {
            HandlerFailure::permanent(format!("remote inbox actor could not be resolved: {error}"))
        }
        _ => HandlerFailure::retry(format!("remote inbox actor resolution failed: {error}")),
    }
}

fn inbox_actor_domain(actor_uri: &str) -> Option<String> {
    let url = Url::parse(actor_uri).ok()?;
    canonical_remote_domain_from_url(&url).ok()
}

fn same_url_origin(left: &Url, right: &Url) -> bool {
    left.scheme().eq_ignore_ascii_case(right.scheme())
        && left.host_str().is_some_and(|left_host| {
            right
                .host_str()
                .is_some_and(|right_host| left_host.eq_ignore_ascii_case(right_host))
        })
        && left.port_or_known_default() == right.port_or_known_default()
}

fn remote_thread_fetch_failure(error: &RemoteFetchError) -> HandlerFailure {
    match error {
        RemoteFetchError::UnexpectedStatus(status)
            if *status == StatusCode::NOT_FOUND
                || *status == StatusCode::REQUEST_TIMEOUT
                || *status == StatusCode::TOO_MANY_REQUESTS
                || status.is_server_error() =>
        {
            HandlerFailure::retry(format!(
                "remote reply parent fetch is temporarily unavailable: {error}"
            ))
        }
        RemoteFetchError::UnexpectedStatus(_) => {
            HandlerFailure::permanent(format!("remote reply parent fetch was rejected: {error}"))
        }
        RemoteFetchError::InvalidUrl
        | RemoteFetchError::Redirect
        | RemoteFetchError::TooManyRedirects
        | RemoteFetchError::MissingContentType
        | RemoteFetchError::UnsupportedContentType
        | RemoteFetchError::UnsupportedEncoding
        | RemoteFetchError::BodyTooLarge
        | RemoteFetchError::InvalidRepresentation
        | RemoteFetchError::IdentityMismatch
        | RemoteFetchError::OriginMismatch
        | RemoteFetchError::Signing
        | RemoteFetchError::BlockedAddress(_) => {
            HandlerFailure::permanent(format!("remote reply parent is invalid: {error}"))
        }
        RemoteFetchError::NoAddresses
        | RemoteFetchError::Dns
        | RemoteFetchError::Client
        | RemoteFetchError::Request
        | RemoteFetchError::BodyRead
        | RemoteFetchError::DomainBudgetExceeded => {
            HandlerFailure::retry(format!("remote reply parent fetch failed: {error}"))
        }
    }
}

fn remote_announce_fetch_failure(error: &RemoteFetchError) -> HandlerFailure {
    match error {
        RemoteFetchError::UnexpectedStatus(status)
            if *status == StatusCode::NOT_FOUND
                || *status == StatusCode::REQUEST_TIMEOUT
                || *status == StatusCode::TOO_MANY_REQUESTS
                || status.is_server_error() =>
        {
            HandlerFailure::retry(format!(
                "remote Announce target fetch is temporarily unavailable: {error}"
            ))
        }
        RemoteFetchError::UnexpectedStatus(_) => HandlerFailure::permanent(format!(
            "remote Announce target fetch was rejected: {error}"
        )),
        RemoteFetchError::InvalidUrl
        | RemoteFetchError::Redirect
        | RemoteFetchError::TooManyRedirects
        | RemoteFetchError::MissingContentType
        | RemoteFetchError::UnsupportedContentType
        | RemoteFetchError::UnsupportedEncoding
        | RemoteFetchError::BodyTooLarge
        | RemoteFetchError::InvalidRepresentation
        | RemoteFetchError::IdentityMismatch
        | RemoteFetchError::OriginMismatch
        | RemoteFetchError::Signing
        | RemoteFetchError::BlockedAddress(_) => {
            HandlerFailure::permanent(format!("remote Announce target is invalid: {error}"))
        }
        RemoteFetchError::NoAddresses
        | RemoteFetchError::Dns
        | RemoteFetchError::Client
        | RemoteFetchError::Request
        | RemoteFetchError::BodyRead
        | RemoteFetchError::DomainBudgetExceeded => {
            HandlerFailure::retry(format!("remote Announce target fetch failed: {error}"))
        }
    }
}

fn remote_thread_write_failure(error: &WriteError) -> HandlerFailure {
    match error {
        WriteError::Sqlx(_)
        | WriteError::Job(_)
        | WriteError::Filesystem(_)
        | WriteError::NotFound => HandlerFailure::retry("remote reply thread persistence failed"),
        WriteError::Conflict
        | WriteError::InvalidInput(_)
        | WriteError::Unauthorized
        | WriteError::RateLimited
        | WriteError::Validation(_) => {
            HandlerFailure::permanent("remote reply thread payload is invalid")
        }
    }
}

fn remote_uri_value(value: Option<&Value>) -> Option<&str> {
    match value {
        Some(Value::String(value)) => Some(value.as_str()),
        Some(Value::Object(object)) => object.get("id").and_then(Value::as_str),
        Some(Value::Array(values)) => remote_uri_value(values.first()),
        _ => None,
    }
}

fn announce_resolution_logical_key(activity_uri: &str) -> String {
    let digest = Sha256::digest(activity_uri.as_bytes());
    let mut digest_string = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut digest_string, "{byte:02x}").expect("writing to a String cannot fail");
    }
    format!("activitypub:announce:{digest_string}")
}

fn note_resolution_logical_key(
    source_account_id: i64,
    actor_uri: &str,
    object_uri: &str,
    delivery_target_account_id: Option<i64>,
) -> String {
    let digest = Sha256::digest(
        format!(
            "{source_account_id}\n{actor_uri}\n{object_uri}\n{}",
            delivery_target_account_id.map_or_else(|| "shared".to_owned(), |id| id.to_string())
        )
        .as_bytes(),
    );
    let mut digest_string = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut digest_string, "{byte:02x}").expect("writing to a String cannot fail");
    }
    format!("activitypub:note:{digest_string}")
}

fn validate_create_binding(
    activity_uri: &str,
    actor_uri: &str,
    object_uri: &str,
) -> Result<(), HandlerFailure> {
    Url::parse(activity_uri)
        .map_err(|_| HandlerFailure::permanent("remote Create activity URI is invalid"))?;
    let actor_location = Url::parse(actor_uri)
        .map_err(|_| HandlerFailure::permanent("remote Create actor URI is invalid"))?;
    let object_location = Url::parse(object_uri)
        .map_err(|_| HandlerFailure::permanent("remote Create object URI is invalid"))?;
    let actor_host = actor_location.host_str();
    if actor_host.is_none()
        || object_location
            .host_str()
            .zip(actor_host)
            .is_none_or(|(object_host, actor_host)| !object_host.eq_ignore_ascii_case(actor_host))
    {
        return Err(HandlerFailure::permanent(
            "remote Create actor and object hosts do not match",
        ));
    }
    Ok(())
}

fn resolved_create_note(
    document: &Value,
    activity_uri: &str,
    actor_uri: &str,
    object_uri: &str,
) -> Result<Value, HandlerFailure> {
    validate_create_binding(activity_uri, actor_uri, object_uri)?;
    if document.get("type").and_then(Value::as_str) != Some("Note") {
        return Err(HandlerFailure::permanent(
            "remote Create object is not a Note",
        ));
    }
    if remote_uri_value(document.get("id")) != Some(object_uri) {
        return Err(HandlerFailure::permanent(
            "remote Create object ID does not match the requested URI",
        ));
    }
    let object = document
        .as_object()
        .ok_or_else(|| HandlerFailure::permanent("remote Create Note is not an object"))?;
    validate_note_object(actor_uri, object)
        .map_err(|_| HandlerFailure::permanent("remote Create Note is invalid"))?;
    Ok(document.clone())
}

#[allow(clippy::too_many_arguments)]
async fn schedule_remote_note_resolution(
    pool: &PgPool,
    source_account_id: i64,
    activity_uri: &str,
    actor_uri: &str,
    object_uri: &str,
    to: &[String],
    cc: &[String],
    delivery_target_account_id: Option<i64>,
    activity: &Value,
) -> Result<(), HandlerFailure> {
    let job = JobSpec::new(
        Lane::Pull,
        ACTIVITYPUB_NOTE_RESOLVE_JOB_KIND,
        json!({
            "source_account_id": source_account_id,
            "activity_uri": activity_uri,
            "actor_uri": actor_uri,
            "object_uri": object_uri,
            "to": to,
            "cc": cc,
            "delivery_target_account_id": delivery_target_account_id,
            "activity": activity
        }),
    )
    .logical_key(note_resolution_logical_key(
        source_account_id,
        actor_uri,
        object_uri,
        delivery_target_account_id,
    ));
    let mut transaction = pool
        .begin()
        .await
        .map_err(|_| HandlerFailure::retry("remote Note resolution outbox transaction failed"))?;
    record_outbox_once_in(&mut transaction, &job)
        .await
        .map_err(|_| HandlerFailure::retry("remote Note resolution outbox write failed"))?;
    transaction
        .commit()
        .await
        .map_err(|_| HandlerFailure::retry("remote Note resolution outbox commit failed"))?;
    Ok(())
}

fn remote_note_fetch_failure(error: &RemoteFetchError) -> HandlerFailure {
    match remote_announce_fetch_failure(error).disposition {
        FailureDisposition::Retry => {
            HandlerFailure::retry(format!("remote Create Note fetch failed: {error}"))
        }
        FailureDisposition::Permanent => {
            HandlerFailure::permanent(format!("remote Create Note fetch is invalid: {error}"))
        }
    }
}

#[allow(clippy::too_many_lines)]
async fn process_activitypub_note_resolution(
    pool: PgPool,
    config: &ActivityPubDeliveryConfig,
    fetcher: &RemoteFetcher,
    arguments: &Value,
) -> Result<(), HandlerFailure> {
    let source_account_id = arguments
        .get("source_account_id")
        .and_then(Value::as_i64)
        .ok_or_else(|| HandlerFailure::permanent("Note resolution job is missing its source"))?;
    let activity_uri = arguments
        .get("activity_uri")
        .and_then(Value::as_str)
        .ok_or_else(|| HandlerFailure::permanent("Note resolution job is missing its activity"))?;
    let actor_uri = arguments
        .get("actor_uri")
        .and_then(Value::as_str)
        .ok_or_else(|| HandlerFailure::permanent("Note resolution job is missing its actor"))?;
    let object_uri = arguments
        .get("object_uri")
        .and_then(Value::as_str)
        .ok_or_else(|| HandlerFailure::permanent("Note resolution job is missing its object"))?;
    let activity = arguments
        .get("activity")
        .ok_or_else(|| HandlerFailure::permanent("Note resolution job is missing its payload"))?;
    let parsed_activity = parse_activity(&activity.to_string())
        .map_err(|_| HandlerFailure::permanent("Note resolution activity is invalid"))?;
    let InboxActivity::CreateNoteReference {
        activity_uri: parsed_activity_uri,
        actor_uri: parsed_actor_uri,
        object_uri: parsed_object_uri,
        to,
        cc,
        ..
    } = parsed_activity
    else {
        return Err(HandlerFailure::permanent(
            "Note resolution payload is not a URI-only Create",
        ));
    };
    if parsed_activity_uri != activity_uri
        || parsed_actor_uri != actor_uri
        || parsed_object_uri != object_uri
        || arguments.get("to") != Some(&json!(to))
        || arguments.get("cc") != Some(&json!(cc))
    {
        return Err(HandlerFailure::permanent(
            "Note resolution contract does not match its activity",
        ));
    }
    validate_create_binding(activity_uri, actor_uri, object_uri)?;
    let delivery_target_account_id = parse_delivery_target_account_id(arguments)?;
    let writer = WriteRepository::from_pool(pool.clone());
    let resolved = writer
        .remote_note_reference_is_resolved(source_account_id, actor_uri, object_uri)
        .await
        .map_err(|error| {
            remote_note_write_failure(&error, "remote Note resolution state lookup failed")
        })?;
    if resolved {
        if let Some(delivery_target_account_id) = delivery_target_account_id {
            writer
                .ensure_remote_note_reference_delivery(
                    source_account_id,
                    actor_uri,
                    object_uri,
                    delivery_target_account_id,
                )
                .await
                .map_err(|error| {
                    remote_note_write_failure(&error, "remote Note delivery target repair failed")
                })?;
        }
        if activity
            .get("signature")
            .is_some_and(|signature| !signature.is_null())
        {
            writer
                .record_remote_note_reference_forwarding(actor_uri, object_uri, activity)
                .await
                .map_err(|error| {
                    remote_note_write_failure(&error, "resolved remote Note forwarding failed")
                })?;
        }
        return Ok(());
    }
    let target = Url::parse(object_uri)
        .map_err(|_| HandlerFailure::permanent("remote Create object URI is invalid"))?;
    if same_url_origin(&target, &config.origin) {
        return Err(HandlerFailure::permanent(
            "remote Create object URI is local",
        ));
    }
    let object_domain = inbox_actor_domain(object_uri)
        .ok_or_else(|| HandlerFailure::permanent("remote Create object has no valid domain"))?;
    if !Repository::from_pool(pool.clone())
        .remote_domain_allowed(&object_domain, config.limited_federation)
        .await
        .map_err(|_| HandlerFailure::retry("remote Create object policy lookup failed"))?
    {
        return Err(HandlerFailure::permanent(
            "remote Create object domain is not allowed",
        ));
    }
    let signer_account = resolve_note_fetch_signer(
        &pool,
        source_account_id,
        delivery_target_account_id,
        &to,
        &cc,
        &config.origin,
    )
    .await?;
    let private_key = signer_account
        .private_key
        .as_ref()
        .filter(|key| key.is_present())
        .ok_or_else(|| HandlerFailure::permanent("remote Create signer has no private key"))?;
    let signer_key_id = format!(
        "{}#main-key",
        activitypub::actor_url(&config.origin, &signer_account)
    );
    let signer = HttpSignatureSigner {
        key_id: &signer_key_id,
        private_key_pem: private_key.as_str(),
    };
    let response = {
        #[cfg(feature = "test-support")]
        if let Some(endpoint) = config.remote_fetch_endpoint {
            fetcher
                .get_for_test_endpoint(target.clone(), THREAD_ACTIVITYPUB_CONTENT_TYPES, endpoint)
                .await
        } else {
            fetcher
                .get_signed(target.clone(), THREAD_ACTIVITYPUB_CONTENT_TYPES, &signer)
                .await
        }
        #[cfg(not(feature = "test-support"))]
        fetcher
            .get_signed(target.clone(), THREAD_ACTIVITYPUB_CONTENT_TYPES, &signer)
            .await
    }
    .map_err(|error| remote_note_fetch_failure(&error))?;
    if !same_url_origin(&response.url, &target) {
        return Err(HandlerFailure::permanent(
            "remote Create object redirected to another origin",
        ));
    }
    let document = serde_json::from_slice::<Value>(&response.body)
        .map_err(|_| HandlerFailure::permanent("remote Create object JSON is invalid"))?;
    let object = resolved_create_note(&document, activity_uri, actor_uri, object_uri)?;
    if !writer
        .remote_note_is_relevant(
            source_account_id,
            actor_uri,
            &object,
            delivery_target_account_id,
            config.origin.as_str(),
        )
        .await
        .map_err(|error| remote_note_write_failure(&error, "remote Note relevance check failed"))?
    {
        return Ok(());
    }
    writer
        .apply_remote_note_create(
            source_account_id,
            actor_uri,
            &object,
            delivery_target_account_id,
            config.origin.as_str(),
        )
        .await
        .map_err(|error| remote_note_write_failure(&error, "resolved remote Note write failed"))?;
    if activity
        .get("signature")
        .is_some_and(|signature| !signature.is_null())
    {
        writer
            .record_remote_note_reference_forwarding(actor_uri, object_uri, activity)
            .await
            .map_err(|error| {
                remote_note_write_failure(&error, "resolved remote Note forwarding failed")
            })?;
    }
    Ok(())
}

fn parse_delivery_target_account_id(arguments: &Value) -> Result<Option<i64>, HandlerFailure> {
    match arguments.get("delivery_target_account_id") {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_i64()
            .map(Some)
            .ok_or_else(|| HandlerFailure::permanent("Note resolution delivery target is invalid")),
    }
}

fn preferred_note_fetch_signer_id(
    delivery_target_account_id: Option<i64>,
    addressed_account_id: Option<i64>,
    follower_account_id: Option<i64>,
) -> Option<i64> {
    delivery_target_account_id
        .or(addressed_account_id)
        .or(follower_account_id)
}

fn note_fetch_audience<'a>(to: &'a [String], cc: &'a [String]) -> Vec<&'a String> {
    to.iter().chain(cc).collect()
}

async fn resolve_note_fetch_signer(
    pool: &PgPool,
    source_account_id: i64,
    delivery_target_account_id: Option<i64>,
    to: &[String],
    cc: &[String],
    origin: &Url,
) -> Result<Account, HandlerFailure> {
    let valid_delivery_target = sqlx::query_scalar::<_, i64>(
        "SELECT id FROM accounts
          WHERE id = $1 AND domain IS NULL
            AND private_key IS NOT NULL AND private_key <> ''",
    )
    .bind(delivery_target_account_id)
    .fetch_optional(pool)
    .await
    .map_err(|_| HandlerFailure::retry("remote Create delivery signer lookup failed"))?;
    let audience = note_fetch_audience(to, cc);
    let addressed_account_id = sqlx::query_scalar::<_, i64>(
        "SELECT account.id
           FROM unnest($1::text[]) WITH ORDINALITY AS audience(uri, position)
           JOIN accounts account ON account.domain IS NULL
             AND account.private_key IS NOT NULL AND account.private_key <> ''
             AND (
               account.uri = audience.uri OR account.url = audience.uri
               OR (audience.uri = $2 || '/actor' AND account.id = -99)
               OR audience.uri = $2 || '/@' || account.username
               OR (audience.uri = $2 || '/users/' || account.username
                   AND account.id_scheme IS DISTINCT FROM 1)
               OR (audience.uri = $2 || '/ap/users/' || account.id::text
                   AND account.id_scheme = 1)
             )
          ORDER BY audience.position, account.id LIMIT 1",
    )
    .bind(&audience)
    .bind(origin.as_str().trim_end_matches('/'))
    .fetch_optional(pool)
    .await
    .map_err(|_| HandlerFailure::retry("remote Create audience signer lookup failed"))?;
    let follower_account_id = sqlx::query_scalar::<_, i64>(
        "SELECT follower.id
           FROM follows follow
           JOIN accounts follower ON follower.id = follow.account_id
          WHERE follow.target_account_id = $1 AND follower.domain IS NULL
            AND follower.private_key IS NOT NULL AND follower.private_key <> ''
          ORDER BY follow.id LIMIT 1",
    )
    .bind(source_account_id)
    .fetch_optional(pool)
    .await
    .map_err(|_| HandlerFailure::retry("remote Create follower signer lookup failed"))?;
    let signer_account_id = preferred_note_fetch_signer_id(
        valid_delivery_target,
        addressed_account_id,
        follower_account_id,
    )
    .unwrap_or(-99);
    Repository::from_pool(pool.clone())
        .account(signer_account_id)
        .await
        .map_err(|_| HandlerFailure::retry("remote Create signer account lookup failed"))?
        .filter(|account| {
            account
                .private_key
                .as_ref()
                .is_some_and(crate::mastodon::SecretText::is_present)
        })
        .ok_or_else(|| HandlerFailure::permanent("remote Create signer is unavailable"))
}

fn remote_announce_audience(arguments: &Value, field: &str) -> Result<Vec<String>, HandlerFailure> {
    let Some(value) = arguments.get(field) else {
        return Ok(Vec::new());
    };
    let values: Vec<&Value> = match value {
        Value::Null => Vec::new(),
        Value::Array(values) => values.iter().collect(),
        value => vec![value],
    };
    if values.len() > 100 {
        return Err(HandlerFailure::permanent(
            "remote Announce resolution audience is too large",
        ));
    }
    values
        .iter()
        .map(|value| {
            remote_uri_value(Some(value))
                .filter(|value| !value.trim().is_empty())
                .map(ToOwned::to_owned)
                .ok_or_else(|| {
                    HandlerFailure::permanent(
                        "remote Announce resolution audience contains an invalid URI",
                    )
                })
        })
        .collect()
}

// A fetched wrapper is authenticated by its transport origin, not its claimed actor.
// Check this before allowing any embedded object to reach a writer.
fn validate_fetched_activity_actor(
    activity_uri: &str,
    actor_uri: &str,
) -> Result<(), HandlerFailure> {
    let matches_origin = Url::parse(activity_uri)
        .ok()
        .zip(Url::parse(actor_uri).ok())
        .is_some_and(|(activity, actor)| same_url_origin(&activity, &actor));
    if !matches_origin {
        return Err(HandlerFailure::permanent(
            "remote fetched activity actor does not match the requested origin",
        ));
    }
    Ok(())
}

fn remote_note_document(
    document: &Value,
    object_uri: &str,
) -> Result<(Value, String, String), HandlerFailure> {
    let (object, actor_uri, target_uri) = match document.get("type").and_then(Value::as_str) {
        Some("Note") => {
            let actor_uri = remote_uri_value(document.get("attributedTo")).ok_or_else(|| {
                HandlerFailure::permanent("remote Announce target Note has no author")
            })?;
            if remote_uri_value(document.get("id")) != Some(object_uri) {
                return Err(HandlerFailure::permanent(
                    "remote Announce target ID does not match the requested URI",
                ));
            }
            (document, actor_uri, object_uri)
        }
        Some("Create") => {
            if remote_uri_value(document.get("id")) != Some(object_uri) {
                return Err(HandlerFailure::permanent(
                    "remote Announce target Create ID does not match the requested URI",
                ));
            }
            let object = document.get("object").ok_or_else(|| {
                HandlerFailure::permanent("remote Announce target Create has no object")
            })?;
            if object.get("type").and_then(Value::as_str) != Some("Note") {
                return Err(HandlerFailure::permanent(
                    "remote Announce target Create object is not a Note",
                ));
            }
            let actor_uri = remote_uri_value(document.get("actor")).ok_or_else(|| {
                HandlerFailure::permanent("remote Announce target Create has no actor")
            })?;
            validate_fetched_activity_actor(object_uri, actor_uri)?;
            if remote_uri_value(object.get("attributedTo")) != Some(actor_uri) {
                return Err(HandlerFailure::permanent(
                    "remote Announce target Note author does not match Create actor",
                ));
            }
            let target_uri = remote_uri_value(object.get("id")).ok_or_else(|| {
                HandlerFailure::permanent("remote Announce target Note has no ID")
            })?;
            (object, actor_uri, target_uri)
        }
        _ => {
            return Err(HandlerFailure::permanent(
                "remote Announce target is not a Note or Create",
            ));
        }
    };
    let object_map = object
        .as_object()
        .ok_or_else(|| HandlerFailure::permanent("remote Announce target Note is not an object"))?;
    validate_note_object(actor_uri, object_map)
        .map_err(|_| HandlerFailure::permanent("remote Announce target Note is invalid"))?;
    Ok((object.clone(), actor_uri.to_owned(), target_uri.to_owned()))
}

#[derive(Debug)]
enum RemoteAnnounceTarget {
    Note {
        object: Value,
        actor_uri: String,
        status_uri: String,
    },
    Announce {
        activity_uri: String,
        actor_uri: String,
        object_uri: String,
        embedded_note: Option<Value>,
        to: Vec<String>,
        cc: Vec<String>,
        published_at: Option<String>,
    },
}

fn remote_announce_document(
    document: &Value,
    object_uri: &str,
) -> Result<RemoteAnnounceTarget, HandlerFailure> {
    if document.get("type").and_then(Value::as_str) != Some("Announce") {
        let (object, actor_uri, status_uri) = remote_note_document(document, object_uri)?;
        return Ok(RemoteAnnounceTarget::Note {
            object,
            actor_uri,
            status_uri,
        });
    }
    let activity_uri = remote_uri_value(document.get("id"))
        .ok_or_else(|| HandlerFailure::permanent("remote nested Announce has no activity ID"))?;
    if activity_uri != object_uri {
        return Err(HandlerFailure::permanent(
            "remote nested Announce ID does not match the requested URI",
        ));
    }
    let actor_uri = remote_uri_value(document.get("actor"))
        .ok_or_else(|| HandlerFailure::permanent("remote nested Announce has no actor"))?;
    validate_fetched_activity_actor(object_uri, actor_uri)?;
    let nested_object = document
        .get("object")
        .ok_or_else(|| HandlerFailure::permanent("remote nested Announce has no object"))?;
    let nested_object_uri = remote_uri_value(Some(nested_object))
        .ok_or_else(|| HandlerFailure::permanent("remote nested Announce object has no URI"))?;
    let embedded_note = nested_object.as_object().and_then(|object| {
        (object.get("type").and_then(Value::as_str) == Some("Note")).then(|| {
            let note_actor_uri = remote_uri_value(object.get("attributedTo"))?;
            // Only self-boosts inherit the wrapper's authority. Foreign authors must
            // be resolved through their canonical object URI, even on the same server.
            (note_actor_uri == actor_uri
                && remote_uri_value(object.get("id")) == Some(nested_object_uri)
                && validate_note_object(note_actor_uri, object).is_ok())
            .then(|| nested_object.clone())
        })?
    });
    Ok(RemoteAnnounceTarget::Announce {
        activity_uri: activity_uri.to_owned(),
        actor_uri: actor_uri.to_owned(),
        object_uri: nested_object_uri.to_owned(),
        embedded_note,
        to: remote_announce_audience(document, "to")?,
        cc: remote_announce_audience(document, "cc")?,
        published_at: document
            .get("published")
            .map(|value| {
                value.as_str().map(ToOwned::to_owned).ok_or_else(|| {
                    HandlerFailure::permanent("remote nested Announce timestamp is invalid")
                })
            })
            .transpose()?,
    })
}

async fn fetch_remote_announce_target(
    config: &ActivityPubDeliveryConfig,
    fetcher: &RemoteFetcher,
    object_url: Url,
    signer: &HttpSignatureSigner<'_>,
) -> Result<RemoteAnnounceTarget, HandlerFailure> {
    #[cfg(not(feature = "test-support"))]
    let _ = config;
    let response = {
        #[cfg(feature = "test-support")]
        if let Some(endpoint) = config.remote_fetch_endpoint {
            fetcher
                .get_for_test_endpoint(
                    object_url.clone(),
                    THREAD_ACTIVITYPUB_CONTENT_TYPES,
                    endpoint,
                )
                .await
        } else {
            fetcher
                .get_signed(object_url.clone(), THREAD_ACTIVITYPUB_CONTENT_TYPES, signer)
                .await
        }
        #[cfg(not(feature = "test-support"))]
        {
            fetcher
                .get_signed(object_url.clone(), THREAD_ACTIVITYPUB_CONTENT_TYPES, signer)
                .await
        }
    }
    .map_err(|error| remote_announce_fetch_failure(&error))?;
    if !same_url_origin(&response.url, &object_url) {
        return Err(HandlerFailure::permanent(
            "remote Announce target redirected to another origin",
        ));
    }
    let document = serde_json::from_slice::<Value>(&response.body)
        .map_err(|_| HandlerFailure::permanent("remote Announce target JSON is invalid"))?;
    remote_announce_document(&document, object_url.as_str())
}

async fn resolve_remote_note_author(
    pool: &PgPool,
    config: &ActivityPubDeliveryConfig,
    writer: &WriteRepository,
    fetcher: &RemoteFetcher,
    actor_uri: &str,
    signer: &HttpSignatureSigner<'_>,
) -> Result<i64, HandlerFailure> {
    if let Some(account_id) = sqlx::query_scalar::<_, i64>(
        "SELECT id FROM accounts WHERE uri = $1 AND domain IS NOT NULL ORDER BY id LIMIT 1",
    )
    .bind(actor_uri)
    .fetch_optional(pool)
    .await
    .map_err(|_| HandlerFailure::retry("remote Announce target author lookup failed"))?
    {
        return Ok(account_id);
    }
    let parsed_actor_url = Url::parse(actor_uri)
        .map_err(|_| HandlerFailure::permanent("remote Announce target author URI is invalid"))?;
    let actor_domain = inbox_actor_domain(actor_uri).ok_or_else(|| {
        HandlerFailure::permanent("remote Announce target author has no valid domain")
    })?;
    if same_url_origin(&parsed_actor_url, &config.origin) {
        return Err(HandlerFailure::permanent(
            "remote Announce target author is local",
        ));
    }
    if !Repository::from_pool(pool.clone())
        .remote_domain_allowed(&actor_domain, config.limited_federation)
        .await
        .map_err(|_| HandlerFailure::retry("remote Announce target author policy lookup failed"))?
    {
        return Err(HandlerFailure::permanent(
            "remote Announce target author domain is not allowed",
        ));
    }
    let actor = RemoteAccountResolver::new(fetcher.clone())
        .resolve_actor_uri_with_signer(&parsed_actor_url, Some(signer))
        .await
        .map_err(|error| remote_announce_fetch_failure(&error))?;
    writer
        .upsert_remote_actor(
            &actor.username,
            &actor_domain,
            config.limited_federation,
            &actor,
        )
        .await
        .map_err(|error| remote_thread_write_failure(&error))
}

async fn resolve_announce_fetch_signer(
    pool: &PgPool,
    source_account_id: i64,
    delivery_target_account_id: Option<i64>,
) -> Result<Account, HandlerFailure> {
    let repository = Repository::from_pool(pool.clone());
    let preferred_account_id = if let Some(delivery_target_account_id) = delivery_target_account_id
    {
        sqlx::query_scalar::<_, i64>(
            "SELECT id FROM accounts
              WHERE id = $1 AND domain IS NULL AND private_key IS NOT NULL
                AND private_key <> ''",
        )
        .bind(delivery_target_account_id)
        .fetch_optional(pool)
        .await
        .map_err(|_| HandlerFailure::retry("remote Announce signer lookup failed"))?
    } else {
        None
    };
    let follower_account_id = sqlx::query_scalar::<_, i64>(
        "SELECT follower.id
               FROM follows follow
               JOIN accounts follower ON follower.id = follow.account_id
              WHERE follow.target_account_id = $1 AND follower.domain IS NULL
                AND follower.private_key IS NOT NULL AND follower.private_key <> ''
              ORDER BY follow.id DESC
              LIMIT 1",
    )
    .bind(source_account_id)
    .fetch_optional(pool)
    .await
    .map_err(|_| HandlerFailure::retry("remote Announce follower signer lookup failed"))?;
    let signer_account_id = follower_account_id.or(preferred_account_id);
    if let Some(signer_account_id) = signer_account_id
        && let Some(account) = repository
            .account(signer_account_id)
            .await
            .map_err(|_| HandlerFailure::retry("remote Announce signer account lookup failed"))?
        && account
            .private_key
            .as_ref()
            .is_some_and(crate::mastodon::SecretText::is_present)
    {
        return Ok(account);
    }
    repository
        .account(-99)
        .await
        .map_err(|_| HandlerFailure::retry("remote Announce instance lookup failed"))?
        .ok_or_else(|| HandlerFailure::permanent("remote Announce instance actor is missing"))
}

const MAX_REMOTE_ANNOUNCE_RESOLUTION_DEPTH: u8 = 4;

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn materialize_remote_announce_target(
    pool: &PgPool,
    config: &ActivityPubDeliveryConfig,
    writer: &WriteRepository,
    fetcher: &RemoteFetcher,
    object_uri: &str,
    embedded_note: Option<Value>,
    signer: &HttpSignatureSigner<'_>,
    delivery_target_account_id: Option<i64>,
    depth: u8,
) -> Result<Option<String>, HandlerFailure> {
    if depth > MAX_REMOTE_ANNOUNCE_RESOLUTION_DEPTH {
        return Err(HandlerFailure::permanent(
            "remote Announce target nesting is too deep",
        ));
    }
    if writer
        .remote_announce_target_exists(object_uri, config.origin.as_str())
        .await
        .map_err(|error| {
            remote_note_write_failure(&error, "remote Announce target lookup failed")
        })?
    {
        return Ok(Some(object_uri.to_owned()));
    }
    let target_url = Url::parse(object_uri)
        .map_err(|_| HandlerFailure::permanent("remote Announce target URI is invalid"))?;
    if same_url_origin(&target_url, &config.origin) {
        return Ok(None);
    }
    let object_domain = inbox_actor_domain(object_uri).ok_or_else(|| {
        HandlerFailure::permanent("remote Announce target URI has no valid domain")
    })?;
    if !Repository::from_pool(pool.clone())
        .remote_domain_allowed(&object_domain, config.limited_federation)
        .await
        .map_err(|_| HandlerFailure::retry("remote Announce target policy lookup failed"))?
    {
        return Err(HandlerFailure::permanent(
            "remote Announce target domain is not allowed",
        ));
    }
    let target = if let Some(note) = embedded_note {
        let (object, actor_uri, status_uri) = remote_note_document(&note, object_uri)?;
        RemoteAnnounceTarget::Note {
            object,
            actor_uri,
            status_uri,
        }
    } else {
        fetch_remote_announce_target(config, fetcher, target_url, signer).await?
    };
    match target {
        RemoteAnnounceTarget::Note {
            object,
            actor_uri,
            status_uri,
        } => {
            let account_id =
                resolve_remote_note_author(pool, config, writer, fetcher, &actor_uri, signer)
                    .await?;
            let written = writer
                .apply_remote_note_create(
                    account_id,
                    &actor_uri,
                    &object,
                    delivery_target_account_id,
                    config.origin.as_str(),
                )
                .await
                .map_err(|error| {
                    remote_note_write_failure(&error, "fetched remote Announce Note write failed")
                })?;
            Ok(written.map(|_| status_uri))
        }
        RemoteAnnounceTarget::Announce {
            activity_uri,
            actor_uri,
            object_uri: nested_object_uri,
            embedded_note,
            to,
            cc,
            published_at,
        } => {
            let Some(nested_status_uri) = Box::pin(materialize_remote_announce_target(
                pool,
                config,
                writer,
                fetcher,
                &nested_object_uri,
                embedded_note,
                signer,
                delivery_target_account_id,
                depth + 1,
            ))
            .await?
            else {
                return Ok(None);
            };
            let account_id =
                resolve_remote_note_author(pool, config, writer, fetcher, &actor_uri, signer)
                    .await?;
            let written = writer
                .apply_remote_announce(
                    account_id,
                    &actor_uri,
                    &activity_uri,
                    &nested_status_uri,
                    &to,
                    &cc,
                    published_at.as_deref(),
                    config.origin.as_str(),
                )
                .await
                .map_err(|error| {
                    remote_note_write_failure(&error, "nested remote Announce write failed")
                })?;
            Ok(written.map(|_| activity_uri))
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn schedule_remote_announce_resolution(
    pool: &PgPool,
    source_account_id: i64,
    actor_uri: &str,
    activity_uri: &str,
    target_uri: &str,
    to: &[String],
    cc: &[String],
    published_at: Option<&str>,
    delivery_target_account_id: Option<i64>,
) -> Result<(), HandlerFailure> {
    let job = JobSpec::new(
        Lane::Pull,
        ACTIVITYPUB_ANNOUNCE_RESOLVE_JOB_KIND,
        json!({
            "source_account_id": source_account_id,
            "actor_uri": actor_uri,
            "activity_uri": activity_uri,
            "object_uri": target_uri,
            "to": to,
            "cc": cc,
            "published_at": published_at,
            "delivery_target_account_id": delivery_target_account_id
        }),
    )
    .logical_key(announce_resolution_logical_key(activity_uri))
    .max_attempts(4);
    let mut transaction = pool.begin().await.map_err(|_| {
        HandlerFailure::retry("remote Announce resolution outbox transaction failed")
    })?;
    record_outbox_once_in(&mut transaction, &job)
        .await
        .map_err(|_| HandlerFailure::retry("remote Announce resolution outbox write failed"))?;
    transaction
        .commit()
        .await
        .map_err(|_| HandlerFailure::retry("remote Announce resolution outbox commit failed"))?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn process_remote_announce(
    pool: &PgPool,
    config: &ActivityPubDeliveryConfig,
    writer: &WriteRepository,
    source_account_id: i64,
    actor_uri: &str,
    activity_uri: &str,
    target_uri: &str,
    embedded_note: Option<Value>,
    to: &[String],
    cc: &[String],
    published_at: Option<&str>,
    delivery_target_account_id: Option<i64>,
) -> Result<(), HandlerFailure> {
    if writer
        .remote_announce_is_tombstoned(source_account_id, activity_uri)
        .await
        .map_err(|error| {
            remote_note_write_failure(&error, "remote Announce tombstone lookup failed")
        })?
    {
        return Ok(());
    }
    if !writer
        .remote_announce_is_relevant(source_account_id, delivery_target_account_id)
        .await
        .map_err(|error| {
            remote_note_write_failure(&error, "remote Announce relevance check failed")
        })?
    {
        return Ok(());
    }
    if writer
        .remote_announce_target_exists(target_uri, config.origin.as_str())
        .await
        .map_err(|error| {
            remote_note_write_failure(&error, "remote Announce target lookup failed")
        })?
    {
        writer
            .apply_remote_announce(
                source_account_id,
                actor_uri,
                activity_uri,
                target_uri,
                to,
                cc,
                published_at,
                config.origin.as_str(),
            )
            .await
            .map_err(|error| remote_note_write_failure(&error, "remote Announce write failed"))?;
        return Ok(());
    }
    let parsed_target = Url::parse(target_uri)
        .map_err(|_| HandlerFailure::permanent("remote Announce target URI is invalid"))?;
    if same_url_origin(&parsed_target, &config.origin) {
        return Ok(());
    }
    if let Some(note) = embedded_note
        && remote_uri_value(note.get("attributedTo")) == Some(actor_uri)
    {
        writer
            .apply_remote_note_create(
                source_account_id,
                actor_uri,
                &note,
                delivery_target_account_id,
                config.origin.as_str(),
            )
            .await
            .map_err(|error| {
                remote_note_write_failure(&error, "embedded remote Announce Note write failed")
            })?;
    } else {
        schedule_remote_announce_resolution(
            pool,
            source_account_id,
            actor_uri,
            activity_uri,
            target_uri,
            to,
            cc,
            published_at,
            delivery_target_account_id,
        )
        .await?;
        return Ok(());
    }
    writer
        .apply_remote_announce(
            source_account_id,
            actor_uri,
            activity_uri,
            target_uri,
            to,
            cc,
            published_at,
            config.origin.as_str(),
        )
        .await
        .map_err(|error| remote_note_write_failure(&error, "remote Announce write failed"))?;
    Ok(())
}

#[allow(clippy::too_many_lines)]
async fn process_activitypub_announce_resolution(
    pool: PgPool,
    config: &ActivityPubDeliveryConfig,
    fetcher: &RemoteFetcher,
    arguments: &Value,
) -> Result<(), HandlerFailure> {
    let source_account_id = arguments
        .get("source_account_id")
        .and_then(Value::as_i64)
        .ok_or_else(|| {
            HandlerFailure::permanent("Announce resolution job is missing its source")
        })?;
    let actor_uri = arguments
        .get("actor_uri")
        .and_then(Value::as_str)
        .ok_or_else(|| HandlerFailure::permanent("Announce resolution job is missing its actor"))?;
    let activity_uri = arguments
        .get("activity_uri")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            HandlerFailure::permanent("Announce resolution job is missing its activity")
        })?;
    let object_uri = arguments
        .get("object_uri")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            HandlerFailure::permanent("Announce resolution job is missing its object")
        })?;
    let to = remote_announce_audience(arguments, "to")?;
    let cc = remote_announce_audience(arguments, "cc")?;
    let published_at = arguments.get("published_at").and_then(Value::as_str);
    let delivery_target_account_id = arguments
        .get("delivery_target_account_id")
        .and_then(Value::as_i64);
    let writer = WriteRepository::from_pool(pool.clone());
    if writer
        .remote_announce_is_tombstoned(source_account_id, activity_uri)
        .await
        .map_err(|error| {
            remote_note_write_failure(&error, "remote Announce tombstone lookup failed")
        })?
        || !writer
            .remote_announce_is_relevant(source_account_id, delivery_target_account_id)
            .await
            .map_err(|error| {
                remote_note_write_failure(&error, "remote Announce relevance check failed")
            })?
    {
        return Ok(());
    }
    if writer
        .remote_announce_target_exists(object_uri, config.origin.as_str())
        .await
        .map_err(|error| {
            remote_note_write_failure(&error, "remote Announce target lookup failed")
        })?
    {
        writer
            .apply_remote_announce(
                source_account_id,
                actor_uri,
                activity_uri,
                object_uri,
                &to,
                &cc,
                published_at,
                config.origin.as_str(),
            )
            .await
            .map_err(|error| remote_note_write_failure(&error, "remote Announce write failed"))?;
        return Ok(());
    }
    let signer_account =
        resolve_announce_fetch_signer(&pool, source_account_id, delivery_target_account_id).await?;
    let private_key = signer_account
        .private_key
        .as_ref()
        .filter(|key| key.is_present())
        .ok_or_else(|| HandlerFailure::permanent("remote Announce signer has no private key"))?;
    let signer_key_id = format!(
        "{}#main-key",
        activitypub::actor_url(&config.origin, &signer_account)
    );
    let signer = HttpSignatureSigner {
        key_id: &signer_key_id,
        private_key_pem: private_key.as_str(),
    };
    let Some(target_status_uri) = materialize_remote_announce_target(
        &pool,
        config,
        &writer,
        fetcher,
        object_uri,
        None,
        &signer,
        delivery_target_account_id,
        0,
    )
    .await?
    else {
        return Ok(());
    };
    writer
        .apply_remote_announce(
            source_account_id,
            actor_uri,
            activity_uri,
            &target_status_uri,
            &to,
            &cc,
            published_at,
            config.origin.as_str(),
        )
        .await
        .map_err(|error| remote_note_write_failure(&error, "remote Announce write failed"))?;
    Ok(())
}

#[allow(clippy::too_many_lines)]
async fn process_activitypub_thread_resolution(
    pool: PgPool,
    config: &ActivityPubDeliveryConfig,
    fetcher: &RemoteFetcher,
    arguments: &Value,
) -> Result<(), HandlerFailure> {
    let child_status_id = arguments
        .get("child_status_id")
        .and_then(Value::as_i64)
        .ok_or_else(|| {
            HandlerFailure::permanent("thread resolution job is missing its child ID")
        })?;
    let parent_uri = arguments
        .get("parent_url")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            HandlerFailure::permanent("thread resolution job is missing its parent URL")
        })?;
    let parsed_parent_url = Url::parse(parent_uri)
        .map_err(|_| HandlerFailure::permanent("thread resolution parent URL is invalid"))?;
    let writer = WriteRepository::from_pool(pool.clone());
    if writer
        .resolve_remote_note_thread(child_status_id, parent_uri, config.origin.as_str())
        .await
        .map_err(|error| remote_thread_write_failure(&error))?
    {
        return Ok(());
    }
    if same_url_origin(&parsed_parent_url, &config.origin) {
        return Err(HandlerFailure::retry(
            "remote reply parent has not arrived locally",
        ));
    }
    let remote_domain = inbox_actor_domain(parent_uri).ok_or_else(|| {
        HandlerFailure::permanent("thread resolution parent URL has no valid remote domain")
    })?;
    if !Repository::from_pool(pool.clone())
        .remote_domain_allowed(&remote_domain, config.limited_federation)
        .await
        .map_err(|_| HandlerFailure::retry("thread resolution domain policy lookup failed"))?
    {
        return Err(HandlerFailure::permanent(
            "thread resolution parent domain is not allowed",
        ));
    }
    let instance = Repository::from_pool(pool.clone())
        .account(-99)
        .await
        .map_err(|_| HandlerFailure::retry("thread resolution instance lookup failed"))?
        .ok_or_else(|| HandlerFailure::permanent("thread resolution instance actor is missing"))?;
    let private_key = instance
        .private_key
        .as_ref()
        .filter(|key| key.is_present())
        .ok_or_else(|| {
            HandlerFailure::permanent("thread resolution instance has no private key")
        })?;
    let signer_key_id = format!(
        "{}#main-key",
        activitypub::actor_url(&config.origin, &instance)
    );
    let signer = HttpSignatureSigner {
        key_id: &signer_key_id,
        private_key_pem: private_key.as_str(),
    };
    let response = fetcher
        .get_signed(
            parsed_parent_url.clone(),
            THREAD_ACTIVITYPUB_CONTENT_TYPES,
            &signer,
        )
        .await
        .map_err(|error| remote_thread_fetch_failure(&error))?;
    if !same_url_origin(&response.url, &parsed_parent_url) {
        return Err(HandlerFailure::permanent(
            "thread resolution parent redirected to another origin",
        ));
    }
    let document = serde_json::from_slice::<Value>(&response.body)
        .map_err(|_| HandlerFailure::permanent("thread resolution parent JSON is invalid"))?;
    let (object, actor_uri) = match document.get("type").and_then(Value::as_str) {
        Some("Note") => {
            let actor_uri = remote_uri_value(document.get("attributedTo"))
                .ok_or_else(|| HandlerFailure::permanent("thread resolution Note has no author"))?;
            (&document, actor_uri)
        }
        Some("Create") => {
            let object = document.get("object").ok_or_else(|| {
                HandlerFailure::permanent("thread resolution Create has no object")
            })?;
            if object.get("type").and_then(Value::as_str) != Some("Note") {
                return Err(HandlerFailure::permanent(
                    "thread resolution Create object is not a Note",
                ));
            }
            let actor_uri = remote_uri_value(document.get("actor")).ok_or_else(|| {
                HandlerFailure::permanent("thread resolution Create has no actor")
            })?;
            if remote_uri_value(object.get("attributedTo")) != Some(actor_uri) {
                return Err(HandlerFailure::permanent(
                    "thread resolution Note author does not match Create actor",
                ));
            }
            (object, actor_uri)
        }
        _ => {
            return Err(HandlerFailure::permanent(
                "thread resolution parent is not a Note or Create",
            ));
        }
    };
    if remote_uri_value(object.get("id")) != Some(parent_uri) {
        return Err(HandlerFailure::permanent(
            "thread resolution parent ID does not match the requested URI",
        ));
    }
    let parsed_actor_url = Url::parse(actor_uri)
        .map_err(|_| HandlerFailure::permanent("thread resolution actor URI is invalid"))?;
    let actor_domain = inbox_actor_domain(actor_uri).ok_or_else(|| {
        HandlerFailure::permanent("thread resolution actor URI has no valid domain")
    })?;
    if !Repository::from_pool(pool.clone())
        .remote_domain_allowed(&actor_domain, config.limited_federation)
        .await
        .map_err(|_| HandlerFailure::retry("thread resolution actor policy lookup failed"))?
    {
        return Err(HandlerFailure::permanent(
            "thread resolution actor domain is not allowed",
        ));
    }
    let actor_account_id = sqlx::query_scalar::<_, i64>(
        "SELECT id FROM accounts WHERE uri = $1 AND domain IS NOT NULL ORDER BY id LIMIT 1",
    )
    .bind(actor_uri)
    .fetch_optional(&pool)
    .await
    .map_err(|_| HandlerFailure::retry("thread resolution actor lookup failed"))?;
    let actor_account_id = if let Some(actor_account_id) = actor_account_id {
        actor_account_id
    } else {
        let actor = RemoteAccountResolver::new(fetcher.clone())
            .resolve_actor_uri_with_signer(&parsed_actor_url, Some(&signer))
            .await
            .map_err(|error| remote_thread_fetch_failure(&error))?;
        writer
            .upsert_remote_actor(
                &actor.username,
                &actor_domain,
                config.limited_federation,
                &actor,
            )
            .await
            .map_err(|error| remote_thread_write_failure(&error))?
    };
    writer
        .apply_remote_note_create(
            actor_account_id,
            actor_uri,
            object,
            None,
            config.origin.as_str(),
        )
        .await
        .map_err(|error| remote_thread_write_failure(&error))?;
    if writer
        .resolve_remote_note_thread(child_status_id, parent_uri, config.origin.as_str())
        .await
        .map_err(|error| remote_thread_write_failure(&error))?
    {
        Ok(())
    } else {
        Err(HandlerFailure::retry(
            "fetched remote reply parent was not persisted",
        ))
    }
}

#[allow(clippy::too_many_lines)]
async fn process_activitypub_emoji(
    pool: PgPool,
    queue: Queue,
    config: &ActivityPubDeliveryConfig,
    fetcher: &RemoteFetcher,
    media_root: PaperclipRoot,
    arguments: &Value,
) -> Result<(), HandlerFailure> {
    let emoji_id = arguments
        .get("emoji_id")
        .and_then(Value::as_i64)
        .ok_or_else(|| HandlerFailure::permanent("emoji fetch job has no emoji ID"))?;
    let expected_url = arguments
        .get("remote_url")
        .and_then(Value::as_str)
        .ok_or_else(|| HandlerFailure::permanent("emoji fetch job has no URL"))?;
    let owner_domain = arguments
        .get("domain")
        .and_then(Value::as_str)
        .ok_or_else(|| HandlerFailure::permanent("emoji fetch job has no domain"))?;
    let advertised_type = arguments
        .get("media_type")
        .filter(|value| !value.is_null())
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| HandlerFailure::permanent("emoji media type is invalid"))
        })
        .transpose()?;
    let remote_url = Url::parse(expected_url)
        .map_err(|_| HandlerFailure::permanent("emoji fetch URL is invalid"))?;
    let image_domain = canonical_remote_domain_from_url(&remote_url)
        .map_err(|_| HandlerFailure::permanent("emoji fetch URL has no valid domain"))?;
    let repository = Repository::from_pool(pool.clone());
    for domain in [owner_domain, image_domain.as_str()] {
        if !repository
            .remote_media_allowed(domain, config.limited_federation)
            .await
            .map_err(|_| HandlerFailure::retry("emoji domain policy lookup failed"))?
        {
            return Err(HandlerFailure::permanent(
                "remote emoji domain is not allowed",
            ));
        }
    }
    let current = sqlx::query_as::<_, (String, String)>(
        "SELECT domain, image_remote_url FROM custom_emojis WHERE id = $1",
    )
    .bind(emoji_id)
    .fetch_optional(&pool)
    .await
    .map_err(|_| HandlerFailure::retry("remote emoji lookup failed"))?;
    let Some((domain, current_url)) = current else {
        return Ok(());
    };
    if domain != owner_domain || current_url != expected_url {
        return Ok(());
    }
    let fetcher = fetcher.with_limits(RemoteFetchLimits {
        max_response_bytes: 256 * 1024,
        ..RemoteFetchLimits::default()
    });
    #[cfg(feature = "test-support")]
    let response = match config.remote_media_endpoint {
        Some(endpoint) => {
            fetcher
                .get_for_test_endpoint(remote_url.clone(), REMOTE_MEDIA_CONTENT_TYPES, endpoint)
                .await
        }
        None => {
            fetcher
                .get(remote_url.clone(), REMOTE_MEDIA_CONTENT_TYPES)
                .await
        }
    };
    #[cfg(not(feature = "test-support"))]
    let response = fetcher
        .get(remote_url.clone(), REMOTE_MEDIA_CONTENT_TYPES)
        .await;
    let response = response.map_err(|error| remote_media_fetch_failure(&error))?;
    let final_image_domain = canonical_remote_domain_from_url(&response.url)
        .map_err(|_| HandlerFailure::permanent("emoji response URL has no valid domain"))?;
    if !repository
        .remote_media_allowed(&final_image_domain, config.limited_federation)
        .await
        .map_err(|_| HandlerFailure::retry("emoji response domain policy lookup failed"))?
    {
        return Err(HandlerFailure::permanent(
            "remote emoji response domain is not allowed",
        ));
    }
    let content_type = response
        .content_type
        .as_deref()
        .and_then(|value| value.split(';').next())
        .map(str::trim)
        .map(str::to_ascii_lowercase)
        .filter(|value| matches!(value.as_str(), "image/png" | "image/gif" | "image/webp"))
        .ok_or_else(|| HandlerFailure::permanent("remote emoji content type is invalid"))?;
    if advertised_type.is_some_and(|advertised| !advertised.eq_ignore_ascii_case(&content_type)) {
        return Err(HandlerFailure::permanent(
            "remote emoji content type does not match its metadata",
        ));
    }
    let source_name = remote_url
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .filter(|name| !name.is_empty())
        .unwrap_or("emoji");
    let prepared = prepare_custom_emoji(emoji_id, source_name, &content_type, &response.body)
        .map_err(|error| HandlerFailure::permanent(format!("remote emoji is invalid: {error}")))?;
    let metadata = PaperclipMetadata {
        attachment: PaperclipAttachment::CustomEmojiImage,
        id: emoji_id,
        remote: true,
        storage_schema_version: Some(1),
        file_name: prepared.file_name.clone(),
        content_type: Some(prepared.content_type.clone()),
        variant: None,
    };
    let mut transaction = pool
        .begin()
        .await
        .map_err(|_| HandlerFailure::retry("remote emoji transaction failed"))?;
    let current_metadata = sqlx::query_as::<_, (Option<String>, Option<String>, Option<i32>)>(
        "SELECT image_file_name, image_content_type, image_storage_schema_version
         FROM custom_emojis WHERE id = $1 AND domain = $2 AND image_remote_url = $3 FOR UPDATE",
    )
    .bind(emoji_id)
    .bind(owner_domain)
    .bind(expected_url)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|_| HandlerFailure::retry("remote emoji fence lookup failed"))?;
    let Some((old_file_name, old_content_type, old_storage_schema_version)) = current_metadata
    else {
        transaction
            .rollback()
            .await
            .map_err(|_| HandlerFailure::retry("remote emoji rollback failed"))?;
        return Ok(());
    };
    let writer = WriteRepository::from_pool(pool.clone());
    for domain in [owner_domain, final_image_domain.as_str()] {
        if !writer
            .remote_media_allowed_in_transaction(
                &mut transaction,
                domain,
                config.limited_federation,
            )
            .await
            .map_err(|_| HandlerFailure::retry("emoji install policy lookup failed"))?
        {
            return Err(HandlerFailure::permanent(
                "remote emoji domain became disallowed",
            ));
        }
    }
    let old_metadata = old_file_name.clone().map(|file_name| PaperclipMetadata {
        attachment: PaperclipAttachment::CustomEmojiImage,
        id: emoji_id,
        remote: true,
        storage_schema_version: old_storage_schema_version,
        file_name,
        content_type: old_content_type,
        variant: None,
    });
    let replacing = old_file_name.as_deref() != Some(prepared.file_name.as_str());
    let mut written_files = if replacing {
        let reconciliation_paths = old_metadata
            .iter()
            .chain(std::iter::once(&metadata))
            .flat_map(|metadata| {
                ["original", "static"]
                    .into_iter()
                    .filter_map(|style| metadata.relative_path(style))
            })
            .collect::<BTreeSet<_>>();
        queue
            .enqueue(&JobSpec::new(
                Lane::Maintenance,
                ACTIVITYPUB_EMOJI_CLEANUP_JOB_KIND,
                json!({"emoji_id": emoji_id, "paths": reconciliation_paths}),
            ))
            .await
            .map_err(|_| HandlerFailure::retry("emoji reconciliation could not be queued"))?;
        let paths = write_prepared_custom_emoji(&media_root, &metadata, &prepared)
            .map_err(|_| HandlerFailure::retry("remote emoji file write failed"))?;
        Some(WrittenMediaFiles::new(&media_root, paths))
    } else {
        None
    };
    let updated = sqlx::query(
        "UPDATE custom_emojis SET image_content_type = $3, image_file_name = $4,
             image_file_size = $5, image_storage_schema_version = 1,
             image_updated_at = clock_timestamp(), updated_at = clock_timestamp()
         WHERE id = $1 AND image_remote_url = $2",
    )
    .bind(emoji_id)
    .bind(expected_url)
    .bind(&prepared.content_type)
    .bind(&prepared.file_name)
    .bind(prepared.file_size)
    .execute(&mut *transaction)
    .await
    .map_err(|_| HandlerFailure::retry("remote emoji metadata update failed"))?;
    if updated.rows_affected() == 0 {
        if let Some(files) = &mut written_files {
            files.cleanup();
        }
        transaction
            .rollback()
            .await
            .map_err(|_| HandlerFailure::retry("remote emoji rollback failed"))?;
        return Ok(());
    }
    if let Some(files) = &mut written_files {
        files.disarm();
    }
    // A commit error is ambiguous. The reconciliation job keeps whichever file set the row names.
    #[cfg(feature = "test-support")]
    if media_root.take_commit_before_fault() {
        return Err(HandlerFailure::retry("remote emoji commit failed"));
    }
    transaction
        .commit()
        .await
        .map_err(|_| HandlerFailure::retry("remote emoji commit failed"))?;
    #[cfg(feature = "test-support")]
    if media_root.take_commit_after_fault() {
        return Err(HandlerFailure::retry("remote emoji commit failed"));
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
async fn process_activitypub_media(
    pool: PgPool,
    config: &ActivityPubDeliveryConfig,
    fetcher: &RemoteFetcher,
    media_root: PaperclipRoot,
    arguments: &Value,
    attempt: i32,
    max_attempts: i32,
) -> Result<(), HandlerFailure> {
    let media_id = arguments
        .get("media_id")
        .and_then(Value::as_i64)
        .ok_or_else(|| HandlerFailure::permanent("media job is missing its media ID"))?;
    let media = sqlx::query_as::<
        _,
        (
            Option<i64>,
            String,
            Option<String>,
            Option<String>,
            Option<Value>,
            Option<String>,
            Option<String>,
        ),
    >(
        "SELECT media.account_id, media.remote_url, media.file_file_name,
                media.file_content_type, media.file_meta, media.blurhash, account.domain
           FROM media_attachments media
           JOIN accounts account ON account.id = media.account_id
           JOIN statuses status ON status.id = media.status_id
          WHERE media.id = $1 AND status.deleted_at IS NULL",
    )
    .bind(media_id)
    .fetch_optional(&pool)
    .await
    .map_err(|_| HandlerFailure::retry("remote media lookup failed"))?;
    let Some((
        Some(account_id),
        remote_url,
        existing_file_name,
        existing_content_type,
        remote_meta,
        blurhash,
        account_domain,
    )) = media
    else {
        return Ok(());
    };
    if remote_url.is_empty() || existing_file_name.is_some() {
        return Ok(());
    }
    let Ok(remote_url) = Url::parse(&remote_url) else {
        mark_remote_media_failed(&pool, media_id, &remote_url).await?;
        return Err(HandlerFailure::permanent("remote media URL is invalid"));
    };
    let account_domain = account_domain
        .ok_or_else(|| HandlerFailure::permanent("remote media account has no domain"))?;
    let remote_domain = canonical_remote_domain(&account_domain)
        .map_err(|_| HandlerFailure::permanent("remote media account has no valid domain"))?;
    if !Repository::from_pool(pool.clone())
        .remote_media_allowed(&remote_domain, config.limited_federation)
        .await
        .map_err(|_| HandlerFailure::retry("remote media domain policy lookup failed"))?
    {
        mark_remote_media_failed(&pool, media_id, remote_url.as_str()).await?;
        return Err(HandlerFailure::permanent(
            "remote media domain is not allowed",
        ));
    }
    if !claim_remote_media_processing(&pool, media_id, remote_url.as_str()).await? {
        return Ok(());
    }
    let file_name = remote_url
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .filter(|value| !value.is_empty())
        .unwrap_or("remote-media");
    let limits = RemoteFetchLimits {
        max_response_bytes: REMOTE_MEDIA_MAX_RESPONSE_BYTES,
        ..RemoteFetchLimits::default()
    };
    let accepted_content_types: &[&str] = if existing_content_type.is_some() {
        REMOTE_MEDIA_CONTENT_TYPES
    } else {
        &[]
    };
    let fetcher = fetcher.with_limits(limits);
    #[cfg(feature = "test-support")]
    let media_response = match config.remote_media_endpoint {
        Some(endpoint) => {
            fetcher
                .get_for_test_endpoint(remote_url.clone(), accepted_content_types, endpoint)
                .await
        }
        None => {
            fetcher
                .get(remote_url.clone(), accepted_content_types)
                .await
        }
    };
    #[cfg(not(feature = "test-support"))]
    let media_response = fetcher
        .get(remote_url.clone(), accepted_content_types)
        .await;
    let response = match media_response {
        Ok(response) => response,
        Err(error) => {
            let failure = remote_media_fetch_failure(&error);
            if failure.disposition == FailureDisposition::Permanent {
                mark_remote_media_failed(&pool, media_id, remote_url.as_str()).await?;
            } else if attempt >= max_attempts {
                mark_remote_media_failed(&pool, media_id, remote_url.as_str()).await?;
                return Err(HandlerFailure::permanent(
                    "remote media retries were exhausted",
                ));
            } else {
                set_remote_media_processing(&pool, media_id, remote_url.as_str(), 0).await?;
            }
            return Err(failure);
        }
    };
    let Some(content_type) = response
        .content_type
        .as_deref()
        .and_then(|value| value.split(';').next())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase)
    else {
        mark_remote_media_failed(&pool, media_id, remote_url.as_str()).await?;
        return Err(HandlerFailure::permanent(
            "remote media content type is missing",
        ));
    };
    let prepared =
        match prepare_media_attachment(account_id, file_name, &content_type, &response.body) {
            Ok(prepared) => prepared,
            Err(error) => {
                if matches!(
                    error,
                    crate::paperclip::MediaAttachmentError::UnsupportedContentType
                ) {
                    set_remote_media_processing(&pool, media_id, remote_url.as_str(), 2).await?;
                    return Ok(());
                }
                mark_remote_media_failed(&pool, media_id, remote_url.as_str()).await?;
                return Err(HandlerFailure::permanent(format!(
                    "remote media could not be processed: {error}"
                )));
            }
        };
    let metadata = PaperclipMetadata {
        attachment: PaperclipAttachment::MediaFile,
        id: media_id,
        remote: true,
        storage_schema_version: Some(1),
        file_name: prepared.file_name.clone(),
        content_type: Some(prepared.content_type.clone()),
        variant: None,
    };
    let mut file_meta = prepared.file_meta.clone();
    let source_meta = remote_meta;
    if let (Value::Object(target), Some(Value::Object(source))) = (&mut file_meta, source_meta) {
        for (key, value) in source {
            if !matches!(key.as_str(), "original" | "small") {
                target.insert(key, value);
            }
        }
    }
    let writer = WriteRepository::from_pool(pool.clone());
    let persisted = writer
        .with_remote_domain_locks(&account_domain, || async {
            let mut transaction = pool.begin().await?;
            let current = sqlx::query_as::<_, (String, Option<String>, Option<String>, bool)>(
                "SELECT media.remote_url, media.file_file_name, account.domain,
                        status.deleted_at IS NULL
                   FROM media_attachments media
                   JOIN accounts account ON account.id = media.account_id
                   JOIN statuses status ON status.id = media.status_id
                  WHERE media.id = $1
                  FOR UPDATE OF media, account, status",
            )
            .bind(media_id)
            .fetch_optional(&mut *transaction)
            .await?;
            let Some((current_remote_url, current_file_name, current_domain, active)) = current
            else {
                transaction.rollback().await?;
                return Ok(false);
            };
            if !active
                || current_remote_url != remote_url.as_str()
                || current_domain.as_deref() != Some(account_domain.as_str())
                || current_file_name.is_some()
            {
                transaction.rollback().await?;
                return Ok(false);
            }
            let allowed = writer
                .remote_media_allowed_in_transaction(
                    &mut transaction,
                    &remote_domain,
                    config.limited_federation,
                )
                .await?;
            if !allowed {
                sqlx::query(
                    "UPDATE media_attachments SET processing = 3, updated_at = clock_timestamp()
                      WHERE id = $1 AND remote_url = $2",
                )
                .bind(media_id)
                .bind(remote_url.as_str())
                .execute(&mut *transaction)
                .await?;
                transaction.commit().await?;
                return Err(WriteError::Validation("remote media domain is not allowed"));
            }
            let written_paths = write_prepared_media(&media_root, &metadata, &prepared)?;
            let mut written_files = WrittenMediaFiles::new(&media_root, written_paths);
            let updated = match sqlx::query(
                "UPDATE media_attachments SET processing = 2, file_content_type = $3,
                    file_file_name = $4, file_file_size = $5, file_meta = $6::json,
                    file_storage_schema_version = 1, file_updated_at = clock_timestamp(),
                    blurhash = COALESCE($7, blurhash), updated_at = clock_timestamp()
                  WHERE id = $1 AND status_id IS NOT NULL AND remote_url = $2
                    AND EXISTS (
                        SELECT 1 FROM statuses
                         WHERE statuses.id = media_attachments.status_id
                           AND statuses.deleted_at IS NULL
                    )",
            )
            .bind(media_id)
            .bind(remote_url.as_str())
            .bind(&prepared.content_type)
            .bind(&prepared.file_name)
            .bind(prepared.file_size)
            .bind(file_meta)
            .bind(blurhash.or(prepared.blurhash))
            .execute(&mut *transaction)
            .await
            {
                Ok(updated) => updated,
                Err(error) => {
                    written_files.cleanup();
                    let _ = transaction.rollback().await;
                    return Err(error.into());
                }
            };
            if updated.rows_affected() == 0 {
                written_files.cleanup();
                transaction.commit().await?;
                return Ok(false);
            }
            // A commit error is ambiguous: PostgreSQL may have committed before the connection
            // failed. Keep the files so a retry can reconcile either database outcome.
            written_files.disarm();
            #[cfg(feature = "test-support")]
            if media_root.take_commit_before_fault() {
                return Err(WriteError::Sqlx(sqlx::Error::Protocol(
                    "injected ambiguous metadata commit failure".to_owned(),
                )));
            }
            transaction.commit().await?;
            #[cfg(feature = "test-support")]
            if media_root.take_commit_after_fault() {
                return Err(WriteError::Sqlx(sqlx::Error::Protocol(
                    "injected ambiguous metadata commit result".to_owned(),
                )));
            }
            Ok(true)
        })
        .await;
    match persisted {
        Ok(_) => Ok(()),
        Err(WriteError::Validation("remote media domain is not allowed")) => Err(
            HandlerFailure::permanent("remote media domain is not allowed"),
        ),
        Err(WriteError::Filesystem(_)) => {
            if attempt >= max_attempts {
                mark_remote_media_failed(&pool, media_id, remote_url.as_str()).await?;
                Err(HandlerFailure::permanent(
                    "remote media file retries were exhausted",
                ))
            } else {
                set_remote_media_processing(&pool, media_id, remote_url.as_str(), 0).await?;
                Err(HandlerFailure::retry("remote media file write failed"))
            }
        }
        Err(_) => {
            if attempt >= max_attempts {
                mark_remote_media_failed(&pool, media_id, remote_url.as_str()).await?;
                Err(HandlerFailure::permanent(
                    "remote media metadata retries were exhausted",
                ))
            } else {
                set_remote_media_processing(&pool, media_id, remote_url.as_str(), 0).await?;
                Err(HandlerFailure::retry("remote media metadata update failed"))
            }
        }
    }
}

struct WrittenMediaFiles {
    root: PaperclipRoot,
    paths: Vec<String>,
}

impl WrittenMediaFiles {
    fn new(root: &PaperclipRoot, paths: Vec<String>) -> Self {
        Self {
            root: root.clone(),
            paths,
        }
    }

    fn cleanup(&mut self) {
        for path in self.paths.drain(..) {
            let _ = self.root.remove_file(Path::new(&path));
        }
    }

    fn disarm(&mut self) {
        self.paths.clear();
    }
}

impl Drop for WrittenMediaFiles {
    fn drop(&mut self) {
        self.cleanup();
    }
}

async fn set_remote_media_processing(
    pool: &PgPool,
    media_id: i64,
    remote_url: &str,
    processing: i32,
) -> Result<bool, HandlerFailure> {
    let updated = sqlx::query(
        "UPDATE media_attachments SET processing = $3, updated_at = clock_timestamp()
          WHERE id = $1 AND status_id IS NOT NULL AND remote_url = $2
            AND file_file_name IS NULL
            AND EXISTS (
                SELECT 1 FROM statuses
                 WHERE statuses.id = media_attachments.status_id
                   AND statuses.deleted_at IS NULL
            )",
    )
    .bind(media_id)
    .bind(remote_url)
    .bind(processing)
    .execute(pool)
    .await
    .map_err(|_| HandlerFailure::retry("remote media processing state update failed"))?;
    Ok(updated.rows_affected() != 0)
}

async fn claim_remote_media_processing(
    pool: &PgPool,
    media_id: i64,
    remote_url: &str,
) -> Result<bool, HandlerFailure> {
    // Lease fencing can cancel a handler after this state change, so retries must be able to
    // reclaim a still-unmaterialized attachment that is already marked as processing.
    let updated = sqlx::query(
        "UPDATE media_attachments SET processing = 1, updated_at = clock_timestamp()
          WHERE id = $1 AND status_id IS NOT NULL AND remote_url = $2
            AND file_file_name IS NULL
            AND EXISTS (
                SELECT 1 FROM statuses
                 WHERE statuses.id = media_attachments.status_id
                   AND statuses.deleted_at IS NULL
            )",
    )
    .bind(media_id)
    .bind(remote_url)
    .execute(pool)
    .await
    .map_err(|_| HandlerFailure::retry("remote media processing claim failed"))?;
    Ok(updated.rows_affected() != 0)
}

async fn mark_remote_media_failed(
    pool: &PgPool,
    media_id: i64,
    remote_url: &str,
) -> Result<(), HandlerFailure> {
    set_remote_media_processing(pool, media_id, remote_url, 3)
        .await
        .map(|_| ())
}

fn remote_media_fetch_failure(error: &RemoteFetchError) -> HandlerFailure {
    match error {
        RemoteFetchError::UnexpectedStatus(status)
            if *status != StatusCode::NOT_IMPLEMENTED
                && (*status == StatusCode::UNAUTHORIZED
                    || *status == StatusCode::REQUEST_TIMEOUT
                    || *status == StatusCode::TOO_MANY_REQUESTS
                    || status.is_server_error()) =>
        {
            HandlerFailure::retry(format!(
                "remote media fetch is temporarily unavailable: {error}"
            ))
        }
        RemoteFetchError::NoAddresses
        | RemoteFetchError::Dns
        | RemoteFetchError::Client
        | RemoteFetchError::Request
        | RemoteFetchError::BodyRead
        | RemoteFetchError::DomainBudgetExceeded => {
            HandlerFailure::retry(format!("remote media fetch failed: {error}"))
        }
        _ => HandlerFailure::permanent(format!("remote media is invalid: {error}")),
    }
}

async fn schedule_remote_follow_accept(
    pool: &PgPool,
    repository: &Repository,
    config: &ActivityPubDeliveryConfig,
    source_account_id: i64,
    target_account_id: i64,
    follow_id: i64,
    follow_uri: &str,
) -> Result<(), HandlerFailure> {
    let source_account = repository
        .account(source_account_id)
        .await
        .map_err(|_| HandlerFailure::retry("remote Follow source lookup failed"))?
        .ok_or_else(|| HandlerFailure::permanent("remote Follow source account is missing"))?;
    let target_account = repository
        .account(target_account_id)
        .await
        .map_err(|_| HandlerFailure::retry("remote Follow target lookup failed"))?
        .ok_or_else(|| HandlerFailure::permanent("remote Follow target account is missing"))?;
    if source_account.domain.is_none() || target_account.domain.is_some() {
        return Err(HandlerFailure::permanent(
            "remote Follow acceptance accounts are not local/remote",
        ));
    }
    let remote_domain = source_account
        .domain
        .clone()
        .ok_or_else(|| HandlerFailure::permanent("remote Follow source has no domain"))?;
    let inbox_url = if source_account.inbox_url.is_empty() {
        source_account.shared_inbox_url.clone()
    } else {
        source_account.inbox_url.clone()
    };
    if inbox_url.is_empty() {
        return Err(HandlerFailure::permanent(
            "remote Follow source has no inbox",
        ));
    }
    let body = activitypub::accept(
        &config.origin,
        &target_account,
        follow_id,
        follow_uri,
        &source_account,
    );
    let delivery = JobSpec::new(
        Lane::Push,
        ACTIVITYPUB_DELIVERY_JOB_KIND,
        json!({
            "source_account_id": target_account_id,
            "inbox_url": inbox_url,
            "remote_domain": remote_domain,
            "body": body
        }),
    )
    .logical_key(activitypub::accept_delivery_logical_key(
        follow_id, follow_uri, &inbox_url,
    ));
    let mut transaction = pool
        .begin()
        .await
        .map_err(|_| HandlerFailure::retry("remote Follow acceptance outbox transaction failed"))?;
    let follow_exists = sqlx::query_scalar::<_, i64>(
        "SELECT id FROM follows
           WHERE id = $1 AND account_id = $2 AND target_account_id = $3 AND uri = $4
           FOR UPDATE",
    )
    .bind(follow_id)
    .bind(source_account_id)
    .bind(target_account_id)
    .bind(follow_uri)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|_| HandlerFailure::retry("remote Follow acceptance relationship lookup failed"))?
    .is_some();
    if !follow_exists {
        return Ok(());
    }
    record_outbox_once_in(&mut transaction, &delivery)
        .await
        .map_err(|_| HandlerFailure::retry("remote Follow acceptance outbox write failed"))?;
    transaction
        .commit()
        .await
        .map_err(|_| HandlerFailure::retry("remote Follow acceptance outbox commit failed"))?;
    Ok(())
}

async fn schedule_remote_follow_reject(
    pool: &PgPool,
    repository: &Repository,
    config: &ActivityPubDeliveryConfig,
    source_account_id: i64,
    target_account_id: i64,
    follow_uri: &str,
) -> Result<(), HandlerFailure> {
    let source_account = repository
        .account(source_account_id)
        .await
        .map_err(|_| HandlerFailure::retry("remote Follow source lookup failed"))?
        .ok_or_else(|| HandlerFailure::permanent("remote Follow source account is missing"))?;
    let target_account = repository
        .account(target_account_id)
        .await
        .map_err(|_| HandlerFailure::retry("remote Follow target lookup failed"))?
        .ok_or_else(|| HandlerFailure::permanent("remote Follow target account is missing"))?;
    if source_account.domain.is_none() || target_account.domain.is_some() {
        return Err(HandlerFailure::permanent(
            "remote Follow rejection accounts are not local/remote",
        ));
    }
    let remote_domain = source_account
        .domain
        .clone()
        .ok_or_else(|| HandlerFailure::permanent("remote Follow source has no domain"))?;
    let inbox_url = if source_account.inbox_url.is_empty() {
        source_account.shared_inbox_url.clone()
    } else {
        source_account.inbox_url.clone()
    };
    if inbox_url.is_empty() {
        return Err(HandlerFailure::permanent(
            "remote Follow source has no inbox",
        ));
    }
    let target_uri = activitypub::actor_url(&config.origin, &target_account);
    let body = activitypub::reject_with_uris(
        &target_uri,
        None,
        follow_uri,
        &activitypub::actor_url(&config.origin, &source_account),
    );
    let delivery = JobSpec::new(
        Lane::Push,
        ACTIVITYPUB_DELIVERY_JOB_KIND,
        json!({
            "source_account_id": target_account_id,
            "inbox_url": inbox_url,
            "remote_domain": remote_domain,
            "body": body
        }),
    )
    .logical_key(activitypub::reject_delivery_logical_key(
        target_account_id,
        follow_uri,
        &inbox_url,
    ));
    let mut transaction = pool
        .begin()
        .await
        .map_err(|_| HandlerFailure::retry("remote Follow rejection outbox transaction failed"))?;
    record_outbox_once_in(&mut transaction, &delivery)
        .await
        .map_err(|_| HandlerFailure::retry("remote Follow rejection outbox write failed"))?;
    transaction
        .commit()
        .await
        .map_err(|_| HandlerFailure::retry("remote Follow rejection outbox commit failed"))?;
    Ok(())
}

async fn resolve_inbox_actor(
    repository: &Repository,
    pool: &PgPool,
    config: &ActivityPubDeliveryConfig,
    fetcher: &RemoteFetcher,
    job: &InboxJob,
    actor_uri: &str,
) -> Result<i64, HandlerFailure> {
    let allowed = repository
        .remote_domain_allowed(&job.remote_domain, config.limited_federation)
        .await
        .map_err(|_| HandlerFailure::retry("remote inbox domain policy lookup failed"))?;
    if !allowed {
        return Err(HandlerFailure::permanent(
            "remote inbox domain is not allowed",
        ));
    }
    if inbox_actor_domain(actor_uri)
        .is_none_or(|domain| !domain.eq_ignore_ascii_case(&job.remote_domain))
    {
        return Err(HandlerFailure::permanent(
            "remote inbox actor domain does not match the verified signer",
        ));
    }

    if let Some(key) = repository
        .activitypub_signature_key(&job.signature_key_id, config.origin.as_str())
        .await
        .map_err(|_| HandlerFailure::retry("remote inbox signature key lookup failed"))?
    {
        let account = repository
            .account(key.account_id)
            .await
            .map_err(|_| HandlerFailure::retry("remote inbox actor lookup failed"))?
            .ok_or_else(|| HandlerFailure::permanent("remote inbox signer account is missing"))?;
        if account
            .domain
            .as_deref()
            .is_none_or(|domain| !domain.eq_ignore_ascii_case(&job.remote_domain))
            || account.uri != actor_uri
        {
            return Err(HandlerFailure::permanent(
                "remote inbox actor does not match the verified signer",
            ));
        }
        return Ok(account.id);
    }

    let instance = repository
        .account(-99)
        .await
        .map_err(|_| HandlerFailure::retry("instance actor lookup failed"))?
        .ok_or_else(|| HandlerFailure::permanent("instance actor is missing"))?;
    let private_key = instance
        .private_key
        .as_ref()
        .filter(|key| key.is_present())
        .ok_or_else(|| HandlerFailure::permanent("instance actor has no private key"))?;
    let signer_key_id = format!(
        "{}#main-key",
        activitypub::actor_url(&config.origin, &instance)
    );
    let signer = HttpSignatureSigner {
        key_id: &signer_key_id,
        private_key_pem: private_key.as_str(),
    };
    #[cfg(feature = "test-support")]
    let fetcher = &fetcher
        .clone()
        .with_test_endpoint(config.remote_fetch_endpoint);
    let resolver = RemoteAccountResolver::new(fetcher.clone());
    let resolution = resolver
        .resolve_key(&job.signature_key_id, Some(&signer))
        .await
        .map_err(|error| inbox_remote_failure(&error))?;
    if !resolution.domain.eq_ignore_ascii_case(&job.remote_domain)
        || resolution.actor.id.as_str() != actor_uri
    {
        return Err(HandlerFailure::permanent(
            "resolved remote actor does not match the verified signer",
        ));
    }
    WriteRepository::from_pool(pool.clone())
        .upsert_remote_actor(
            &resolution.actor.username,
            &resolution.domain,
            config.limited_federation,
            &resolution.actor,
        )
        .await
        .map_err(|_| HandlerFailure::retry("remote inbox actor persistence failed"))
}

#[allow(clippy::too_many_lines)]
async fn process_activitypub_inbox(
    pool: PgPool,
    config: &ActivityPubDeliveryConfig,
    fetcher: &RemoteFetcher,
    arguments: &Value,
    report_mail_enabled: bool,
) -> Result<(), HandlerFailure> {
    let job = parse_job_arguments(arguments)
        .map_err(|error| HandlerFailure::permanent(error.to_string()))?;
    let activity =
        parse_activity(&job.body).map_err(|error| HandlerFailure::permanent(error.to_string()))?;
    if matches!(activity, InboxActivity::Unsupported) {
        return Ok(());
    }
    let (actor_uri, nested_actor_uri) = match &activity {
        InboxActivity::CreateNote { actor_uri, .. }
        | InboxActivity::CreateNoteReference { actor_uri, .. }
        | InboxActivity::UpdateNote { actor_uri, .. }
        | InboxActivity::DeleteNote { actor_uri, .. }
        | InboxActivity::Like { actor_uri, .. }
        | InboxActivity::Announce { actor_uri, .. }
        | InboxActivity::UndoLike { actor_uri, .. }
        | InboxActivity::UndoAnnounce { actor_uri, .. }
        | InboxActivity::Follow { actor_uri, .. }
        | InboxActivity::Flag { actor_uri, .. }
        | InboxActivity::Block { actor_uri, .. }
        | InboxActivity::UpdateActor { actor_uri, .. }
        | InboxActivity::DeleteActor { actor_uri, .. }
        | InboxActivity::UndoReference { actor_uri, .. } => (actor_uri.as_str(), None),
        InboxActivity::Accept {
            actor_uri,
            nested_actor_uri,
            ..
        }
        | InboxActivity::Reject {
            actor_uri,
            nested_actor_uri,
            ..
        }
        | InboxActivity::UndoFollow {
            actor_uri,
            nested_actor_uri,
            ..
        }
        | InboxActivity::UndoBlock {
            actor_uri,
            nested_actor_uri,
            ..
        } => (actor_uri.as_str(), nested_actor_uri.as_deref()),
        InboxActivity::Unsupported => unreachable!("unsupported activities return above"),
    };
    if matches!(
        &activity,
        InboxActivity::UndoFollow { .. } | InboxActivity::UndoBlock { .. }
    ) && nested_actor_uri.is_some_and(|nested| nested != actor_uri)
    {
        return Err(HandlerFailure::permanent(
            "remote Undo actor does not match its embedded Follow",
        ));
    }
    let repository = Repository::from_pool(pool.clone());
    let source_account_id =
        resolve_inbox_actor(&repository, &pool, config, fetcher, &job, actor_uri).await?;
    let writer = WriteRepository::from_pool(pool.clone());
    match activity {
        InboxActivity::Like {
            activity_uri,
            actor_uri,
            object_uri,
        } => {
            writer
                .apply_remote_like(
                    source_account_id,
                    &actor_uri,
                    &activity_uri,
                    &object_uri,
                    config.origin.as_str(),
                )
                .await
                .map_err(|error| remote_note_write_failure(&error, "remote Like write failed"))?;
        }
        InboxActivity::UndoLike {
            actor_uri,
            activity_uri,
            object_uri,
        } => {
            writer
                .apply_remote_undo_like(
                    source_account_id,
                    &actor_uri,
                    &activity_uri,
                    &object_uri,
                    config.origin.as_str(),
                )
                .await
                .map_err(|error| {
                    remote_note_write_failure(&error, "remote Undo Like write failed")
                })?;
        }
        InboxActivity::Announce {
            activity_uri,
            actor_uri,
            object_uri,
            embedded_note,
            to,
            cc,
            published_at,
        } => {
            process_remote_announce(
                &pool,
                config,
                &writer,
                source_account_id,
                &actor_uri,
                &activity_uri,
                &object_uri,
                embedded_note,
                &to,
                &cc,
                published_at.as_deref(),
                job.delivery_target_account_id,
            )
            .await?;
        }
        InboxActivity::UndoAnnounce {
            actor_uri,
            activity_uri,
            object_uri,
        } => {
            writer
                .apply_remote_undo_announce(
                    source_account_id,
                    &actor_uri,
                    &activity_uri,
                    &object_uri,
                    config.origin.as_str(),
                )
                .await
                .map_err(|error| {
                    remote_note_write_failure(&error, "remote Undo Announce write failed")
                })?;
        }
        InboxActivity::CreateNote {
            actor_uri,
            object,
            activity: original_activity,
            ..
        } => {
            if !writer
                .remote_note_is_relevant(
                    source_account_id,
                    &actor_uri,
                    &object,
                    job.delivery_target_account_id,
                    config.origin.as_str(),
                )
                .await
                .map_err(|error| {
                    remote_note_write_failure(&error, "remote Note relevance check failed")
                })?
            {
                return Ok(());
            }
            writer
                .apply_remote_note_create(
                    source_account_id,
                    &actor_uri,
                    &object,
                    job.delivery_target_account_id,
                    config.origin.as_str(),
                )
                .await
                .map_err(|error| {
                    remote_note_write_failure(&error, "remote Note Create write failed")
                })?;
            if original_activity
                .get("signature")
                .is_some_and(|signature| !signature.is_null())
            {
                writer
                    .record_remote_note_forwarding(&actor_uri, &object, &original_activity)
                    .await
                    .map_err(|error| {
                        remote_note_write_failure(
                            &error,
                            "remote Note forwarding outbox write failed",
                        )
                    })?;
            }
        }
        InboxActivity::CreateNoteReference {
            activity_uri,
            actor_uri,
            object_uri,
            to,
            cc,
            activity,
        } => {
            schedule_remote_note_resolution(
                &pool,
                source_account_id,
                &activity_uri,
                &actor_uri,
                &object_uri,
                &to,
                &cc,
                job.delivery_target_account_id,
                &activity,
            )
            .await?;
        }
        InboxActivity::UpdateNote {
            actor_uri,
            object,
            activity,
        } => {
            let note_exists = writer
                .remote_note_exists(source_account_id, &actor_uri, &object)
                .await
                .map_err(|error| {
                    remote_note_write_failure(&error, "remote Note existence check failed")
                })?;
            if !note_exists
                && !writer
                    .remote_note_is_relevant(
                        source_account_id,
                        &actor_uri,
                        &object,
                        job.delivery_target_account_id,
                        config.origin.as_str(),
                    )
                    .await
                    .map_err(|error| {
                        remote_note_write_failure(&error, "remote Note relevance check failed")
                    })?
            {
                return Ok(());
            }
            let updated = writer
                .apply_remote_note_update(
                    source_account_id,
                    &actor_uri,
                    &object,
                    job.delivery_target_account_id,
                    config.origin.as_str(),
                )
                .await
                .map_err(|error| {
                    remote_note_write_failure(&error, "remote Note Update write failed")
                })?;
            if note_exists
                && updated.is_some()
                && activity
                    .get("signature")
                    .is_some_and(|signature| !signature.is_null())
            {
                writer
                    .record_remote_note_forwarding(&actor_uri, &object, &activity)
                    .await
                    .map_err(|error| {
                        remote_note_write_failure(
                            &error,
                            "remote Note Update forwarding outbox write failed",
                        )
                    })?;
            }
        }
        InboxActivity::DeleteNote {
            actor_uri,
            object_uri,
            atom_uri,
            activity,
        } => {
            if activity
                .get("signature")
                .is_some_and(|signature| !signature.is_null())
            {
                writer
                    .record_remote_note_delete_forwarding(
                        &actor_uri,
                        &object_uri,
                        atom_uri.as_deref(),
                        &activity,
                    )
                    .await
                    .map_err(|error| {
                        remote_note_write_failure(
                            &error,
                            "remote Note Delete forwarding outbox write failed",
                        )
                    })?;
            }
            let media = writer
                .remote_note_media_metadata(
                    source_account_id,
                    &actor_uri,
                    &object_uri,
                    atom_uri.as_deref(),
                )
                .await
                .map_err(|error| {
                    remote_note_write_failure(&error, "remote Note media lookup failed")
                })?;
            if let Some(media_root) = config.media_root.as_ref() {
                remove_paperclip_files(media_root, &media)?;
            }
            writer
                .apply_remote_note_delete(
                    source_account_id,
                    &actor_uri,
                    &object_uri,
                    atom_uri.as_deref(),
                )
                .await
                .map_err(|error| {
                    remote_note_write_failure(&error, "remote Note Delete write failed")
                })?;
        }
        InboxActivity::Follow {
            activity_uri,
            object_uri,
            ..
        } => {
            let outcome = writer
                .apply_remote_follow(
                    source_account_id,
                    &activity_uri,
                    &object_uri,
                    config.origin.as_str(),
                    job.delivery_target_account_id,
                )
                .await
                .map_err(|_| HandlerFailure::retry("remote Follow write failed"))?;
            if let Some(outcome) = outcome {
                match outcome {
                    RemoteFollowOutcome::Applied(outcome) => {
                        if !outcome.request {
                            schedule_remote_follow_accept(
                                &pool,
                                &repository,
                                config,
                                source_account_id,
                                outcome.recipient_account_id,
                                outcome.activity_id,
                                &activity_uri,
                            )
                            .await?;
                        }
                    }
                    RemoteFollowOutcome::Rejected {
                        recipient_account_id,
                    } => {
                        schedule_remote_follow_reject(
                            &pool,
                            &repository,
                            config,
                            source_account_id,
                            recipient_account_id,
                            &activity_uri,
                        )
                        .await?;
                    }
                }
            }
        }
        InboxActivity::UndoFollow {
            follow_uri,
            target_uri,
            ..
        } => {
            writer
                .apply_remote_undo_follow(
                    source_account_id,
                    &follow_uri,
                    target_uri.as_deref(),
                    config.origin.as_str(),
                    job.delivery_target_account_id,
                )
                .await
                .map_err(|_| HandlerFailure::retry("remote Undo Follow write failed"))?;
        }
        InboxActivity::Block {
            activity_uri,
            object_uri,
            ..
        } => {
            writer
                .apply_remote_block(
                    source_account_id,
                    &activity_uri,
                    &object_uri,
                    config.origin.as_str(),
                    job.delivery_target_account_id,
                )
                .await
                .map_err(|_| HandlerFailure::retry("remote Block write failed"))?;
        }
        InboxActivity::Flag {
            activity_uri,
            object_uris,
            comment,
            ..
        } => {
            writer
                .create_remote_report(
                    source_account_id,
                    &object_uris,
                    &comment,
                    activity_uri.as_deref(),
                    config.origin.as_str(),
                    &config.local_domain,
                    report_mail_enabled,
                )
                .await
                .map_err(|error| {
                    remote_note_write_failure(&error, "remote Flag report write failed")
                })?;
        }
        InboxActivity::UpdateActor { actor_uri, object } => {
            writer
                .apply_remote_actor_update(source_account_id, &actor_uri, &object)
                .await
                .map_err(|_| HandlerFailure::retry("remote actor Update write failed"))?;
        }
        InboxActivity::DeleteActor {
            actor_uri,
            object_uri,
        } => {
            if actor_uri != object_uri {
                return Err(HandlerFailure::permanent(
                    "remote actor Delete object does not match its actor",
                ));
            }
            let media = writer
                .remote_actor_media_metadata(source_account_id, &actor_uri)
                .await
                .map_err(|_| HandlerFailure::retry("remote actor media lookup failed"))?;
            if let Some(media_root) = config.media_root.as_ref() {
                remove_paperclip_files(media_root, &media)?;
            }
            writer
                .apply_remote_actor_delete(
                    source_account_id,
                    &actor_uri,
                    config.origin.as_str(),
                    None,
                    None,
                )
                .await
                .map_err(|_| HandlerFailure::retry("remote actor Delete write failed"))?;
        }
        InboxActivity::UndoBlock {
            block_uri,
            target_uri,
            ..
        } => {
            writer
                .apply_remote_undo_block(
                    source_account_id,
                    &block_uri,
                    target_uri.as_deref(),
                    config.origin.as_str(),
                    job.delivery_target_account_id,
                )
                .await
                .map_err(|_| HandlerFailure::retry("remote Undo Block write failed"))?;
        }
        InboxActivity::Accept {
            follow_uri,
            target_uri,
            nested_actor_uri,
            ..
        } => {
            writer
                .apply_remote_follow_decision(
                    source_account_id,
                    &follow_uri,
                    target_uri.as_deref(),
                    nested_actor_uri.as_deref(),
                    true,
                    config.origin.as_str(),
                    job.delivery_target_account_id,
                )
                .await
                .map_err(|_| HandlerFailure::retry("remote Accept write failed"))?;
        }
        InboxActivity::Reject {
            follow_uri,
            target_uri,
            nested_actor_uri,
            ..
        } => {
            writer
                .apply_remote_follow_decision(
                    source_account_id,
                    &follow_uri,
                    target_uri.as_deref(),
                    nested_actor_uri.as_deref(),
                    false,
                    config.origin.as_str(),
                    job.delivery_target_account_id,
                )
                .await
                .map_err(|_| HandlerFailure::retry("remote Reject write failed"))?;
        }
        InboxActivity::UndoReference {
            actor_uri,
            object_uri,
        } => {
            match writer
                .remote_undo_reference_kind(source_account_id, &object_uri)
                .await
                .map_err(|error| {
                    remote_note_write_failure(&error, "remote Undo reference lookup failed")
                })? {
                RemoteUndoReferenceKind::Follow => writer
                    .apply_remote_undo_follow(
                        source_account_id,
                        &object_uri,
                        None,
                        config.origin.as_str(),
                        job.delivery_target_account_id,
                    )
                    .await
                    .map_err(|_| {
                        HandlerFailure::retry("remote Undo reference follow write failed")
                    })?,
                RemoteUndoReferenceKind::Block => writer
                    .apply_remote_undo_block(
                        source_account_id,
                        &object_uri,
                        None,
                        config.origin.as_str(),
                        job.delivery_target_account_id,
                    )
                    .await
                    .map_err(|_| {
                        HandlerFailure::retry("remote Undo reference block write failed")
                    })?,
                RemoteUndoReferenceKind::Announce | RemoteUndoReferenceKind::Unknown => writer
                    .apply_remote_undo_announce_reference(
                        source_account_id,
                        &actor_uri,
                        &object_uri,
                    )
                    .await
                    .map_err(|error| {
                        remote_note_write_failure(
                            &error,
                            "remote Undo reference interaction write failed",
                        )
                    })?,
            }
        }
        InboxActivity::Unsupported => unreachable!("unsupported activities return above"),
    }
    Ok(())
}

fn remote_note_write_failure(error: &WriteError, message: &str) -> HandlerFailure {
    match error {
        &WriteError::Conflict
        | &WriteError::InvalidInput(_)
        | &WriteError::NotFound
        | &WriteError::Unauthorized
        | &WriteError::RateLimited
        | &WriteError::Validation(_) => HandlerFailure::permanent(message),
        &WriteError::Sqlx(_) | &WriteError::Job(_) | &WriteError::Filesystem(_) => {
            HandlerFailure::retry(message)
        }
    }
}

fn delivery_logical_key(status_id: i64, inbox_url: &str) -> String {
    activitypub::status_delivery_logical_key(status_id, inbox_url)
}

fn update_delivery_logical_key(
    status_id: i64,
    activity_id: &str,
    edited_at_micros: i64,
    inbox_url: &str,
) -> String {
    let digest =
        Sha256::digest(format!("{activity_id}\0{edited_at_micros}\0{inbox_url}").as_bytes());
    let mut digest_string = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut digest_string, "{byte:02x}").expect("writing to a String cannot fail");
    }
    format!("activitypub:status:{status_id}:update:{digest_string}")
}

fn account_update_delivery_logical_key(
    account_id: i64,
    activity_id: &str,
    updated_at_micros: i64,
    inbox_url: &str,
) -> String {
    let digest =
        Sha256::digest(format!("{activity_id}\0{updated_at_micros}\0{inbox_url}").as_bytes());
    let mut digest_string = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut digest_string, "{byte:02x}").expect("writing to a String cannot fail");
    }
    format!("activitypub:account:{account_id}:update:{digest_string}")
}

fn delete_delivery_logical_key(status_id: i64, inbox_url: &str) -> String {
    activitypub::status_delete_delivery_logical_key(status_id, inbox_url)
}

/// Builds the infrastructure handlers available before feature-specific handlers are registered.
///
/// # Errors
///
/// Returns an error if a built-in handler cannot be registered.
pub fn infrastructure_handlers(queue: &Queue) -> Result<HandlerRegistry, WorkerError> {
    infrastructure_handlers_with_writer_and_mail(queue, None, None)
}

/// Builds infrastructure handlers with an optional Mastodon writer pool for maintenance jobs.
///
/// The durable-job queue itself remains on the runtime pool. A writer pool is only used for
/// explicitly configured application-table maintenance, such as timed mute expiry.
///
/// # Errors
///
/// Returns an error if a built-in handler cannot be registered.
pub fn infrastructure_handlers_with_writer(
    queue: &Queue,
    mastodon_writer: Option<PgPool>,
) -> Result<HandlerRegistry, WorkerError> {
    infrastructure_handlers_with_writer_and_mail(queue, mastodon_writer, None)
}

/// Builds infrastructure handlers with optional Mastodon writes and SMTP mail delivery.
///
/// # Errors
///
/// Returns an error if a built-in handler cannot be registered.
pub fn infrastructure_handlers_with_writer_and_mail(
    queue: &Queue,
    mastodon_writer: Option<PgPool>,
    mail_runtime: Option<MailRuntime>,
) -> Result<HandlerRegistry, WorkerError> {
    infrastructure_handlers_with_writer_and_mail_and_federation(
        queue,
        mastodon_writer,
        mail_runtime,
        None,
    )
}

/// Builds infrastructure handlers with optional Mastodon writes, SMTP mail, and federation
/// delivery.
///
/// # Errors
///
/// Returns an error if a built-in handler cannot be registered.
#[allow(clippy::too_many_lines)]
pub fn infrastructure_handlers_with_writer_and_mail_and_federation(
    queue: &Queue,
    mastodon_writer: Option<PgPool>,
    mail_runtime: Option<MailRuntime>,
    federation: Option<ActivityPubDeliveryConfig>,
) -> Result<HandlerRegistry, WorkerError> {
    let handlers = HandlerRegistry::new();
    let pool = queue.pool().clone();
    let report_mail_enabled = mail_runtime.is_some();
    let domain_block_media_root = federation
        .as_ref()
        .and_then(|config| config.media_root.clone());
    if let Some(mastodon_writer) = mastodon_writer {
        if let Some(media_root) = domain_block_media_root.clone() {
            let cleanup_pool = mastodon_writer.clone();
            handlers.register(
                LOCAL_MEDIA_CLEANUP_JOB_KIND,
                Lane::Maintenance,
                ResourceClass::Media,
                move |job| {
                    let pool = cleanup_pool.clone();
                    let root = media_root.clone();
                    async move { process_local_media_cleanup_job(pool, root, &job.arguments).await }
                },
            )?;
        }
        let status_writer = mastodon_writer.clone();
        handlers.register(
            STATUS_NOTIFICATION_JOB_KIND,
            Lane::Core,
            ResourceClass::None,
            move |job| {
                let pool = status_writer.clone();
                async move {
                    let Some(status_id) = job.arguments.get("status_id").and_then(Value::as_i64)
                    else {
                        return Err(HandlerFailure::permanent(
                            "status notification job is missing its status ID",
                        ));
                    };
                    WriteRepository::from_pool(pool)
                        .notify_status_followers(status_id)
                        .await
                        .map(|_| ())
                        .map_err(|_| HandlerFailure::retry("status notification fan-out failed"))
                }
            },
        )?;
        let notification_pool = mastodon_writer.clone();
        handlers.register(
            NOTIFICATION_CREATE_JOB_KIND,
            Lane::Core,
            ResourceClass::None,
            move |job| {
                let pool = notification_pool.clone();
                async move { process_notification_job(pool, &job.arguments).await }
            },
        )?;
        let unfilter_pool = mastodon_writer.clone();
        handlers.register(
            NOTIFICATION_UNFILTER_JOB_KIND,
            Lane::Core,
            ResourceClass::None,
            move |job| {
                let pool = unfilter_pool.clone();
                async move { process_notification_unfilter_job(pool, &job.arguments).await }
            },
        )?;
        let cleanup_pool = mastodon_writer.clone();
        handlers.register(
            NOTIFICATION_CLEANUP_JOB_KIND,
            Lane::Core,
            ResourceClass::None,
            move |job| {
                let pool = cleanup_pool.clone();
                async move { process_notification_cleanup_job(pool, &job.arguments).await }
            },
        )?;
        let maintenance_pool = mastodon_writer.clone();
        handlers.register(
            "rustodon.mastodon.delete_mute",
            Lane::Maintenance,
            ResourceClass::None,
            move |job| {
                let pool = maintenance_pool.clone();
                async move {
                    let Some(mute_id) = job.arguments.get("mute_id").and_then(Value::as_i64) else {
                        return Err(HandlerFailure::permanent(
                            "mute expiry job is missing its mute ID",
                        ));
                    };
                    sqlx::query(
                        "DELETE FROM mutes WHERE id = $1 \
                         AND expires_at IS NOT NULL AND expires_at <= clock_timestamp()",
                    )
                    .bind(mute_id)
                    .execute(&pool)
                    .await
                    .map_err(|_| HandlerFailure::retry("mute expiry cleanup failed"))?;
                    Ok(())
                }
            },
        )?;
        let purge_pool = mastodon_writer.clone();
        let purge_queue = queue.clone();
        let purge_media_root = domain_block_media_root.clone();
        handlers.register(
            MASTODON_ACCOUNT_PURGE_JOB_KIND,
            Lane::Maintenance,
            ResourceClass::Media,
            move |job| {
                let pool = purge_pool.clone();
                let queue = purge_queue.clone();
                let media_root = purge_media_root.clone();
                async move { process_account_purge_job(queue, pool, &job, media_root).await }
            },
        )?;
        let domain_block_pool = mastodon_writer.clone();
        let domain_block_media_root_for_handler = domain_block_media_root.clone();
        handlers.register(
            MASTODON_DOMAIN_BLOCK_JOB_KIND,
            Lane::Maintenance,
            ResourceClass::Media,
            move |job| {
                let pool = domain_block_pool.clone();
                let media_root = domain_block_media_root_for_handler.clone();
                async move { process_domain_block_job(pool, &job.arguments, media_root).await }
            },
        )?;
        let domain_purge_pool = mastodon_writer.clone();
        let domain_purge_media_root = domain_block_media_root;
        handlers.register(
            MASTODON_DOMAIN_PURGE_JOB_KIND,
            Lane::Maintenance,
            ResourceClass::Media,
            move |job| {
                let pool = domain_purge_pool.clone();
                let media_root = domain_purge_media_root.clone();
                async move { process_domain_purge_job(pool, &job.arguments, media_root).await }
            },
        )?;
        if let Some(federation) = federation {
            let remote_fetcher = RemoteFetcher::default().with_operational_pool(pool.clone());
            let note_pool = mastodon_writer.clone();
            let note_config = federation.clone();
            let note_fetcher = remote_fetcher.clone();
            handlers.register(
                ACTIVITYPUB_NOTE_RESOLVE_JOB_KIND,
                Lane::Pull,
                ResourceClass::RemoteHttp,
                move |job| {
                    let pool = note_pool.clone();
                    let config = note_config.clone();
                    let fetcher = note_fetcher.clone();
                    async move {
                        process_activitypub_note_resolution(pool, &config, &fetcher, &job.arguments)
                            .await
                    }
                },
            )?;
            let announce_pool = mastodon_writer.clone();
            let announce_config = federation.clone();
            let announce_fetcher = remote_fetcher.clone();
            handlers.register(
                ACTIVITYPUB_ANNOUNCE_RESOLVE_JOB_KIND,
                Lane::Pull,
                ResourceClass::RemoteHttp,
                move |job| {
                    let pool = announce_pool.clone();
                    let config = announce_config.clone();
                    let fetcher = announce_fetcher.clone();
                    async move {
                        process_activitypub_announce_resolution(
                            pool,
                            &config,
                            &fetcher,
                            &job.arguments,
                        )
                        .await
                    }
                },
            )?;
            let thread_pool = mastodon_writer.clone();
            let thread_config = federation.clone();
            let thread_fetcher = remote_fetcher.clone();
            handlers.register(
                ACTIVITYPUB_THREAD_RESOLVE_JOB_KIND,
                Lane::Pull,
                ResourceClass::RemoteHttp,
                move |job| {
                    let pool = thread_pool.clone();
                    let config = thread_config.clone();
                    let fetcher = thread_fetcher.clone();
                    async move {
                        process_activitypub_thread_resolution(
                            pool,
                            &config,
                            &fetcher,
                            &job.arguments,
                        )
                        .await
                    }
                },
            )?;
            if let Some(media_root) = federation.media_root.clone() {
                let emoji_cleanup_pool = mastodon_writer.clone();
                let emoji_cleanup_root = media_root.clone();
                handlers.register(
                    ACTIVITYPUB_EMOJI_CLEANUP_JOB_KIND,
                    Lane::Maintenance,
                    ResourceClass::Media,
                    move |job| {
                        let pool = emoji_cleanup_pool.clone();
                        let media_root = emoji_cleanup_root.clone();
                        async move {
                            process_activitypub_emoji_cleanup_job(pool, media_root, &job.arguments)
                                .await
                        }
                    },
                )?;
                let emoji_pool = mastodon_writer.clone();
                let emoji_queue = queue.clone();
                let emoji_config = federation.clone();
                let emoji_fetcher = remote_fetcher.clone();
                let emoji_root = media_root.clone();
                handlers.register(
                    ACTIVITYPUB_EMOJI_FETCH_JOB_KIND,
                    Lane::Pull,
                    ResourceClass::Media,
                    move |job| {
                        let pool = emoji_pool.clone();
                        let queue = emoji_queue.clone();
                        let config = emoji_config.clone();
                        let fetcher = emoji_fetcher.clone();
                        let media_root = emoji_root.clone();
                        async move {
                            process_activitypub_emoji(
                                pool,
                                queue,
                                &config,
                                &fetcher,
                                media_root,
                                &job.arguments,
                            )
                            .await
                        }
                    },
                )?;
                let media_pool = mastodon_writer.clone();
                let media_config = federation.clone();
                let media_fetcher = remote_fetcher.clone();
                handlers.register(
                    ACTIVITYPUB_MEDIA_FETCH_JOB_KIND,
                    Lane::Pull,
                    ResourceClass::Media,
                    move |job| {
                        let pool = media_pool.clone();
                        let config = media_config.clone();
                        let fetcher = media_fetcher.clone();
                        let media_root = media_root.clone();
                        async move {
                            process_activitypub_media(
                                pool,
                                &config,
                                &fetcher,
                                media_root,
                                &job.arguments,
                                job.attempt,
                                job.max_attempts,
                            )
                            .await
                        }
                    },
                )?;
            }
            let inbox_pool = mastodon_writer.clone();
            let inbox_config = federation.clone();
            let inbox_fetcher = remote_fetcher.clone();
            handlers.register(
                ACTIVITYPUB_INBOX_JOB_KIND,
                Lane::Ingress,
                ResourceClass::RemoteHttp,
                move |job| {
                    let pool = inbox_pool.clone();
                    let config = inbox_config.clone();
                    let fetcher = inbox_fetcher.clone();
                    async move {
                        process_activitypub_inbox(
                            pool,
                            &config,
                            &fetcher,
                            &job.arguments,
                            report_mail_enabled,
                        )
                        .await
                    }
                },
            )?;
            let account_update_pool = mastodon_writer.clone();
            let account_update_config = federation.clone();
            handlers.register(
                ACTIVITYPUB_ACCOUNT_UPDATE_JOB_KIND,
                Lane::Push,
                ResourceClass::None,
                move |job| {
                    let pool = account_update_pool.clone();
                    let config = account_update_config.clone();
                    async move {
                        let account_id = job
                            .arguments
                            .get("account_id")
                            .and_then(Value::as_i64)
                            .ok_or_else(|| {
                                HandlerFailure::permanent(
                                    "account update job is missing its account ID",
                                )
                            })?;
                        let updated_at_micros = job
                            .arguments
                            .get("updated_at_micros")
                            .and_then(Value::as_i64)
                            .ok_or_else(|| {
                                HandlerFailure::permanent(
                                    "account update job is missing its timestamp",
                                )
                            })?;
                        distribute_account_update(pool, &config, account_id, updated_at_micros)
                            .await
                    }
                },
            )?;
            let account_delete_pool = mastodon_writer.clone();
            let account_delete_config = federation.clone();
            handlers.register(
                ACTIVITYPUB_ACCOUNT_DELETE_JOB_KIND,
                Lane::Push,
                ResourceClass::None,
                move |job| {
                    let pool = account_delete_pool.clone();
                    let config = account_delete_config.clone();
                    async move {
                        let account_id = job
                            .arguments
                            .get("account_id")
                            .and_then(Value::as_i64)
                            .ok_or_else(|| {
                                HandlerFailure::permanent(
                                    "account deletion job is missing its account ID",
                                )
                            })?;
                        let actor_uri = job
                            .arguments
                            .get("actor_uri")
                            .and_then(Value::as_str)
                            .ok_or_else(|| {
                                HandlerFailure::permanent(
                                    "account deletion job is missing its actor URI",
                                )
                            })?;
                        distribute_account_delete(pool, &config, account_id, actor_uri).await
                    }
                },
            )?;
            let distribution_pool = mastodon_writer.clone();
            let distribution_config = federation.clone();
            handlers.register(
                ACTIVITYPUB_STATUS_DISTRIBUTION_JOB_KIND,
                Lane::Push,
                ResourceClass::None,
                move |job| {
                    let pool = distribution_pool.clone();
                    let config = distribution_config.clone();
                    async move {
                        let Some(status_id) =
                            job.arguments.get("status_id").and_then(Value::as_i64)
                        else {
                            return Err(HandlerFailure::permanent(
                                "status distribution job is missing its status ID",
                            ));
                        };
                        let activity_type = job
                            .arguments
                            .get("activity_type")
                            .and_then(Value::as_str)
                            .unwrap_or("Create");
                        let edited_at_micros = job
                            .arguments
                            .get("edited_at_micros")
                            .and_then(Value::as_i64);
                        let explicit_recipient_ids =
                            match job.arguments.get("recipient_account_ids") {
                                None => Vec::new(),
                                Some(Value::Array(values)) => values
                                    .iter()
                                    .map(|value| {
                                        value.as_i64().ok_or_else(|| {
                                        HandlerFailure::permanent(
                                            "status distribution recipient account ID is invalid",
                                        )
                                    })
                                    })
                                    .collect::<Result<Vec<_>, _>>()?,
                                Some(_) => {
                                    return Err(HandlerFailure::permanent(
                                        "status distribution recipient account IDs are invalid",
                                    ));
                                }
                            };
                        distribute_status(
                            pool,
                            &config,
                            status_id,
                            activity_type,
                            edited_at_micros,
                            &explicit_recipient_ids,
                        )
                        .await
                    }
                },
            )?;
            let delivery_pool = mastodon_writer.clone();
            let operational_pool = queue.pool().clone();
            let delivery_fetcher = remote_fetcher.clone();
            handlers.register(
                ACTIVITYPUB_DELIVERY_JOB_KIND,
                Lane::Push,
                ResourceClass::RemoteHttp,
                move |job| {
                    let pool = delivery_pool.clone();
                    let operational_pool = operational_pool.clone();
                    let config = federation.clone();
                    let fetcher = delivery_fetcher.clone();
                    async move {
                        deliver_activity(pool, operational_pool, &config, &fetcher, &job.arguments)
                            .await
                    }
                },
            )?;
        }
    }
    if let Some(mail_runtime) = mail_runtime {
        let mail_runtime = Arc::new(mail_runtime);
        let mail_queue = queue.clone();
        for kind in [
            crate::mail::PASSWORD_RESET_JOB_KIND,
            crate::mail::CONFIRMATION_JOB_KIND,
            crate::mail::REPORT_JOB_KIND,
        ] {
            let mail_runtime = Arc::clone(&mail_runtime);
            let mail_queue = mail_queue.clone();
            handlers.register(kind, Lane::Mail, ResourceClass::None, move |job| {
                let mail_runtime = Arc::clone(&mail_runtime);
                let mail_queue = mail_queue.clone();
                async move {
                    mail_runtime
                        .send_queued(&mail_queue, job)
                        .await
                        .map_err(|error| {
                            if error.retryable() {
                                HandlerFailure::retry("SMTP delivery failed")
                            } else {
                                HandlerFailure::permanent(error.to_string())
                            }
                        })
                }
            })?;
        }
    }
    handlers.register(
        "rustodon.maintenance.prune",
        Lane::Maintenance,
        ResourceClass::None,
        move |_job| {
            let pool = pool.clone();
            async move {
                sqlx::raw_sql(
                    "DELETE FROM rustodon.idempotency_keys WHERE expires_at <= clock_timestamp(); \
                       DELETE FROM rustodon.ordering_markers marker \
                        WHERE marker.expires_at <= clock_timestamp() \
                         AND NOT EXISTS ( \
                           SELECT 1 FROM rustodon.durable_jobs job \
                            WHERE job.dead_at IS NULL \
                              AND (job.arguments ->> '_rustodon_ordering_key' = \
                                   pg_catalog.encode(marker.key_hash, 'hex') \
                                   OR job.id = CASE \
                                     WHEN marker.payload ->> 'job_id' ~ '^[0-9]+$' \
                                     THEN (marker.payload ->> 'job_id')::bigint \
                                     ELSE NULL \
                                   END) \
                          ); \
                       DELETE FROM rustodon.rate_limit_windows \
                         WHERE expires_at <= clock_timestamp(); \
                       DELETE FROM rustodon.remote_fetch_leases \
                         WHERE expires_at <= clock_timestamp(); \
                        DELETE FROM rustodon.heartbeats \
                         WHERE heartbeat_at < clock_timestamp() - interval '1 day'",
                )
                .execute(&pool)
                .await
                .map_err(|_| HandlerFailure::retry("operational cleanup failed"))?;
                Ok(())
            }
        },
    )?;
    Ok(handlers)
}

/// Runs worker and scheduler loops until shutdown, then stops claiming and drains in-flight jobs.
///
/// # Errors
///
/// Returns an error when queue operations fail or workers cannot drain before the configured
/// shutdown deadline.
#[allow(clippy::too_many_lines)]
pub async fn run_until_shutdown<F>(
    queue: Queue,
    handlers: HandlerRegistry,
    config: WorkerConfig,
    process_id: String,
    shutdown: F,
) -> Result<(), WorkerError>
where
    F: Future<Output = ()> + Send,
{
    let remote_http = usize::try_from(config.remote_http_concurrency)
        .map_err(|_| WorkerError::InvalidConfiguration("remote HTTP concurrency is too large"))?;
    let media = usize::try_from(config.media_concurrency)
        .map_err(|_| WorkerError::InvalidConfiguration("media concurrency is too large"))?;
    let supported_lanes = handlers.supported_lanes()?;
    if !config.lanes.is_subset(&supported_lanes) {
        return Err(WorkerError::InvalidConfiguration(
            "a configured worker lane has no registered handler",
        ));
    }
    let lanes = config.lanes.iter().copied().collect::<Vec<_>>();
    let schedules_maintenance = lanes.contains(&Lane::Maintenance);
    let executor = WorkerExecutor::new(queue.clone(), handlers, remote_http, media)?;
    let lease = Duration::seconds(i64::from(config.lease_seconds));
    let poll = StdDuration::from_millis(u64::from(config.poll_milliseconds));
    let heartbeat = StdDuration::from_secs(u64::from(config.heartbeat_seconds));
    let worker_heartbeat_id = format!("{process_id}:worker");
    let scheduler_heartbeat_id = format!("{process_id}:scheduler");
    queue
        .heartbeat(&WorkerHeartbeat::worker(
            &worker_heartbeat_id,
            lanes.iter().copied(),
            json!({"concurrency": config.concurrency}),
        ))
        .await?;
    if let Err(error) = queue
        .heartbeat(&WorkerHeartbeat::scheduler(
            &scheduler_heartbeat_id,
            json!({"outbox": true}),
        ))
        .await
    {
        let _ = queue.remove_heartbeat(&worker_heartbeat_id).await;
        return Err(error.into());
    }
    let (stop_sender, stop_receiver) = watch::channel(false);
    let mut tasks = JoinSet::new();
    for slot in 0..config.concurrency {
        let executor = executor.clone();
        let lanes = lanes.clone();
        let lease_owner = format!("{process_id}:{slot}");
        let mut stop = stop_receiver.clone();
        tasks.spawn(async move {
            loop {
                if *stop.borrow() {
                    return Ok(());
                }
                if !executor.process_one(&lease_owner, &lanes, lease).await? {
                    tokio::select! {
                        () = tokio::time::sleep(poll) => {}
                        result = stop.changed() => {
                            if result.is_err() || *stop.borrow() {
                                return Ok(());
                            }
                        }
                    }
                }
            }
        });
    }
    let scheduler_queue = queue.clone();
    let scheduler_lanes = lanes.clone();
    let scheduler_worker_id = worker_heartbeat_id.clone();
    let scheduler_id = scheduler_heartbeat_id.clone();
    let concurrency = config.concurrency;
    let mut scheduler_stop = stop_receiver.clone();
    let mut scheduler = tokio::spawn(async move {
        let mut heartbeat_tick = tokio::time::interval(heartbeat);
        let mut outbox_tick = tokio::time::interval(poll);
        let mut maintenance_tick = tokio::time::interval(StdDuration::from_mins(1));
        loop {
            tokio::select! {
                result = scheduler_stop.changed() => {
                    if result.is_err() || *scheduler_stop.borrow() {
                        return Ok(());
                    }
                }
                _ = heartbeat_tick.tick() => {
                    scheduler_queue.heartbeat(&WorkerHeartbeat::worker(
                        &scheduler_worker_id,
                        scheduler_lanes.iter().copied(),
                        json!({"concurrency": concurrency}),
                    )).await?;
                    scheduler_queue.heartbeat(&WorkerHeartbeat::scheduler(
                        &scheduler_id,
                        json!({"outbox": true}),
                    )).await?;
                }
                _ = outbox_tick.tick() => {
                    scheduler_queue.dispatch_outbox(100).await?;
                }
                _ = maintenance_tick.tick(), if schedules_maintenance => {
                    let minute = Utc::now().timestamp() / 60;
                    scheduler_queue.enqueue(
                        &JobSpec::new(
                            Lane::Maintenance,
                            "rustodon.maintenance.prune",
                            json!({"minute": minute}),
                        ).logical_key(format!("maintenance:{minute}")),
                    ).await?;
                }
            }
        }
    });

    tokio::pin!(shutdown);
    let mut scheduler_finished = false;
    let run_result = tokio::select! {
        () = &mut shutdown => Ok(()),
        result = tasks.join_next() => match result {
            Some(Ok(result)) => result,
            Some(Err(_)) | None => Err(WorkerError::TaskFailed),
        },
        result = &mut scheduler => {
            scheduler_finished = true;
            match result {
                Ok(result) => result,
                Err(_) => Err(WorkerError::TaskFailed),
            }
        },
    };
    stop_sender.send_replace(true);
    let deadline =
        tokio::time::Instant::now() + StdDuration::from_secs(u64::from(config.shutdown_seconds));
    if !scheduler_finished {
        // Stop and join the sole heartbeat writer before deleting readiness rows.
        scheduler.abort();
        if tokio::time::timeout_at(deadline, &mut scheduler)
            .await
            .is_err()
        {
            tasks.abort_all();
            return Err(WorkerError::ShutdownTimedOut);
        }
    }
    let cleanup_result = tokio::time::timeout_at(deadline, async {
        queue.remove_heartbeat(&worker_heartbeat_id).await?;
        queue.remove_heartbeat(&scheduler_heartbeat_id).await
    })
    .await;
    match cleanup_result {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            tasks.abort_all();
            return Err(error.into());
        }
        Err(_) => {
            tasks.abort_all();
            return Err(WorkerError::ShutdownTimedOut);
        }
    }
    let drain_result = tokio::time::timeout_at(deadline, async {
        let mut drain_error = None;
        while let Some(result) = tasks.join_next().await {
            match result {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    drain_error.get_or_insert(error);
                }
                Err(_) => {
                    drain_error.get_or_insert(WorkerError::TaskFailed);
                }
            }
        }
        drain_error.map_or(Ok(()), Err)
    })
    .await;
    let drain_result = if let Ok(result) = drain_result {
        result
    } else {
        // Cancelling a handler never acknowledges its job. The fenced lease remains durable and
        // another worker will recover it after expiry.
        tasks.abort_all();
        Err(WorkerError::ShutdownTimedOut)
    };
    run_result?;
    drain_result
}

/// Returns a deterministic bounded quartic backoff with per-job jitter.
#[must_use]
pub fn retry_delay(job_id: i64, attempt: i32) -> Duration {
    let attempt = i64::from(attempt.max(1));
    let quartic = attempt.saturating_pow(4);
    let jitter_bound = quartic.saturating_mul(10).max(1);
    let mixed = u64::from_ne_bytes(job_id.to_ne_bytes())
        ^ u64::try_from(attempt)
            .unwrap_or_default()
            .wrapping_mul(0x9e37_79b9_7f4a_7c15);
    let jitter =
        i64::try_from(mixed % u64::try_from(jitter_bound).unwrap_or(u64::MAX)).unwrap_or_default();
    Duration::seconds(
        15_i64
            .saturating_add(quartic.saturating_mul(10))
            .saturating_add(jitter)
            .min(24 * 60 * 60),
    )
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Utc};
    use http::StatusCode;
    use serde_json::json;

    use super::{
        FailureDisposition, HandlerRegistry, RemoteAnnounceTarget, ResourceClass,
        account_purge_cleanup_paths, account_update_delivery_is_current,
        account_update_delivery_logical_key, delivery_failure, delivery_logical_key,
        inbox_actor_domain, note_fetch_audience, note_resolution_logical_key,
        preferred_note_fetch_signer_id, remote_announce_document, remote_media_fetch_failure,
        remote_note_document, remote_note_fetch_failure, resolved_create_note, retry_delay,
        safe_cleanup_path, update_delivery_logical_key, validate_create_binding,
    };
    use crate::jobs::Lane;
    use crate::remote::RemoteFetchError;

    #[test]
    fn retry_backoff_is_deterministic_jittered_and_bounded() {
        assert_eq!(retry_delay(42, 2), retry_delay(42, 2));
        assert_ne!(retry_delay(42, 2), retry_delay(43, 2));
        assert!(retry_delay(42, 1) < retry_delay(42, 2));
        assert!(retry_delay(42, 100).num_hours() <= 24);
    }

    #[test]
    fn account_purge_cleanup_manifest_is_validated_and_deduplicated() {
        let arguments = json!({
            "cleanup_paths": [
                "media_attachments/files/000/000/007/original/file.png",
                "media_attachments/files/000/000/007/original/file.png",
                "accounts/avatars/000/000/007/original/avatar.png"
            ]
        });
        assert_eq!(
            account_purge_cleanup_paths(&arguments).unwrap(),
            Some(vec![
                "accounts/avatars/000/000/007/original/avatar.png".to_owned(),
                "media_attachments/files/000/000/007/original/file.png".to_owned(),
            ])
        );
        assert_eq!(account_purge_cleanup_paths(&json!({})).unwrap(), None);
        assert!(safe_cleanup_path(
            "accounts/avatars/000/original/avatar.png"
        ));
        assert!(!safe_cleanup_path("../outside"));
        assert!(!safe_cleanup_path("/outside"));
        assert!(!safe_cleanup_path("accounts/avatars/\0avatar.png"));
    }

    #[test]
    fn inbox_actor_domains_remove_default_ports_but_preserve_non_default_ports() {
        assert_eq!(
            inbox_actor_domain("http://remote.example:80/users/alice").as_deref(),
            Some("remote.example")
        );
        assert_eq!(
            inbox_actor_domain("https://remote.example:443/users/alice").as_deref(),
            Some("remote.example")
        );
        assert_eq!(
            inbox_actor_domain("http://remote.example:443/users/alice").as_deref(),
            Some("remote.example:443")
        );
        assert_eq!(
            inbox_actor_domain("https://remote.example:8443/users/alice").as_deref(),
            Some("remote.example:8443")
        );
    }

    #[test]
    fn duplicate_registration_preserves_the_original_handler_lane() {
        let registry = HandlerRegistry::new();
        registry
            .register("duplicate", Lane::Core, ResourceClass::None, |_job| async {
                Ok(())
            })
            .unwrap();
        assert!(
            registry
                .register(
                    "duplicate",
                    Lane::Push,
                    ResourceClass::RemoteHttp,
                    |_job| async { Ok(()) },
                )
                .is_err()
        );
        assert_eq!(
            registry.supported_lanes().unwrap(),
            [Lane::Core].into_iter().collect()
        );
    }

    #[test]
    fn delivery_keys_are_stable_and_endpoint_specific() {
        let first = delivery_logical_key(7, "https://remote.example/inbox");
        assert_eq!(
            first,
            delivery_logical_key(7, "https://remote.example/inbox")
        );
        assert_ne!(
            first,
            delivery_logical_key(7, "https://other.example/inbox")
        );
        assert_ne!(
            first,
            delivery_logical_key(8, "https://remote.example/inbox")
        );
        assert!(first.len() <= 1024);
    }

    #[test]
    fn update_delivery_keys_include_the_activity_version() {
        let first = update_delivery_logical_key(
            7,
            "https://local.example/users/alice/statuses/7#updates/1",
            1_000_000,
            "https://remote.example/inbox",
        );
        assert_eq!(
            first,
            update_delivery_logical_key(
                7,
                "https://local.example/users/alice/statuses/7#updates/1",
                1_000_000,
                "https://remote.example/inbox",
            )
        );
        assert_ne!(
            first,
            update_delivery_logical_key(
                7,
                "https://local.example/users/alice/statuses/7#updates/2",
                2_000_000,
                "https://remote.example/inbox",
            )
        );
        assert_ne!(
            first,
            update_delivery_logical_key(
                7,
                "https://local.example/users/alice/statuses/7#updates/1",
                1_000_001,
                "https://remote.example/inbox",
            )
        );
        assert_ne!(
            first,
            update_delivery_logical_key(
                7,
                "https://local.example/users/alice/statuses/7#updates/1",
                1_000_000,
                "https://other.example/inbox",
            )
        );
    }

    #[test]
    fn account_update_delivery_keys_include_the_timestamp_version() {
        let first = account_update_delivery_logical_key(
            7,
            "https://local.example/users/alice#updates/1",
            1_000_000,
            "https://remote.example/inbox",
        );
        assert_ne!(
            first,
            account_update_delivery_logical_key(
                7,
                "https://local.example/users/alice#updates/1",
                1_000_001,
                "https://remote.example/inbox",
            )
        );
        assert_ne!(
            first,
            account_update_delivery_logical_key(
                7,
                "https://local.example/users/alice#updates/1",
                1_000_000,
                "https://other.example/inbox",
            )
        );
    }

    #[test]
    fn account_update_delivery_rejects_stale_microsecond_version_with_same_activity_id() {
        let updated_at = DateTime::<Utc>::from_timestamp(1_700_000_000, 123_000_000)
            .expect("valid timestamp")
            .naive_utc();
        let actor_uri = "https://local.example/users/alice";
        let activity_id = format!("{actor_uri}#updates/{}", updated_at.and_utc().timestamp());
        let current_updated_at_micros = updated_at.and_utc().timestamp_micros();

        assert!(!account_update_delivery_is_current(
            Some(&activity_id),
            actor_uri,
            updated_at,
            Some(current_updated_at_micros - 1),
        ));
        assert!(account_update_delivery_is_current(
            Some(&activity_id),
            actor_uri,
            updated_at,
            Some(current_updated_at_micros),
        ));
    }

    #[test]
    fn remote_create_wrapper_keeps_activity_and_note_uris_distinct() {
        let note_uri = "https://remote.example/users/alice/statuses/1";
        let create_uri = "https://remote.example/activities/1";
        let document = json!({
            "id": create_uri,
            "type": "Create",
            "actor": "https://remote.example/users/alice",
            "object": {
                "id": note_uri,
                "type": "Note",
                "attributedTo": "https://remote.example/users/alice",
                "content": "hello"
            }
        });
        let (object, actor_uri, target_uri) =
            remote_note_document(&document, create_uri).expect("valid Create wrapper");
        assert_eq!(object["id"], note_uri);
        assert_eq!(actor_uri, "https://remote.example/users/alice");
        assert_eq!(target_uri, note_uri);
    }

    #[test]
    fn fetched_wrappers_reject_cross_origin_actor_provenance() {
        for wrapper_type in ["Create", "Announce"] {
            for fetched_uri in [
                "https://evil.example/activities/1",
                "http://remote.example/activities/1",
                "https://remote.example:8443/activities/1",
            ] {
                let document = json!({
                    "id": fetched_uri,
                    "type": wrapper_type,
                    "actor": "https://remote.example/users/alice",
                    "object": {
                        "id": "https://remote.example/users/alice/statuses/1",
                        "type": "Note",
                        "attributedTo": "https://remote.example/users/alice",
                        "content": "forged"
                    }
                });
                assert!(
                    remote_announce_document(&document, fetched_uri).is_err(),
                    "{wrapper_type} must not authenticate a different origin: {fetched_uri}"
                );
            }
        }
    }

    #[test]
    fn note_resolution_keys_preserve_actor_object_and_personal_recipient() {
        let first = note_resolution_logical_key(
            1,
            "https://remote.example/users/alice",
            "https://remote.example/statuses/1",
            Some(7),
        );
        assert_eq!(
            first,
            note_resolution_logical_key(
                1,
                "https://remote.example/users/alice",
                "https://remote.example/statuses/1",
                Some(7),
            )
        );
        assert_ne!(
            first,
            note_resolution_logical_key(
                1,
                "https://remote.example/users/alice",
                "https://remote.example/statuses/1",
                Some(8),
            )
        );
        assert_ne!(
            first,
            note_resolution_logical_key(
                2,
                "https://remote.example/users/mallory",
                "https://remote.example/statuses/2",
                Some(7),
            )
        );
    }

    #[test]
    fn note_fetch_signer_priority_matches_mastodon() {
        assert_eq!(
            preferred_note_fetch_signer_id(Some(1), Some(2), Some(3)),
            Some(1)
        );
        assert_eq!(
            preferred_note_fetch_signer_id(None, Some(2), Some(3)),
            Some(2)
        );
        assert_eq!(preferred_note_fetch_signer_id(None, None, Some(3)), Some(3));
        assert_eq!(preferred_note_fetch_signer_id(None, None, None), None);
    }

    #[test]
    fn note_fetch_signer_considers_shared_inbox_outer_audience_in_order() {
        let to = vec![
            "https://local.example/users/first".to_owned(),
            "https://local.example/users/second".to_owned(),
        ];
        let cc = vec!["https://local.example/users/third".to_owned()];

        assert_eq!(note_fetch_audience(&to, &cc), vec![&to[0], &to[1], &cc[0]]);
    }

    #[test]
    fn resolved_create_note_accepts_cross_host_activity_with_origin_bound_object() {
        let activity_uri = "https://relay.example/activities/1";
        let actor_uri = "https://remote.example/users/alice";
        let object_uri = "https://remote.example/statuses/1";
        let valid = json!({
            "id": object_uri,
            "type": "Note",
            "attributedTo": actor_uri,
            "content": "hello"
        });
        assert!(resolved_create_note(&valid, activity_uri, actor_uri, object_uri).is_ok());
    }

    #[test]
    fn create_binding_rejects_cross_host_object_spoofing() {
        assert_eq!(
            validate_create_binding(
                "https://relay.example/activities/1",
                "https://remote.example/users/alice",
                "https://attacker.example/statuses/1",
            )
            .expect_err("the dereferenced object must remain on the actor origin")
            .disposition,
            FailureDisposition::Permanent
        );
    }

    #[test]
    fn resolved_create_note_rejects_spoofed_logical_identity_actor_and_type() {
        let activity_uri = "https://relay.example/activities/1";
        let actor_uri = "https://remote.example/users/alice";
        let object_uri = "https://remote.example/statuses/1";
        let valid = json!({
            "id": object_uri,
            "type": "Note",
            "attributedTo": actor_uri,
            "content": "hello"
        });

        for spoofed in [
            json!({"id":"https://remote.example/statuses/2","type":"Note","attributedTo":actor_uri,"content":"hello"}),
            json!({"id":object_uri,"type":"Note","attributedTo":"https://remote.example/users/mallory","content":"hello"}),
            json!({"id":object_uri,"type":"Article","attributedTo":actor_uri,"content":"hello"}),
            json!({"id":activity_uri,"type":"Create","actor":actor_uri,"object":valid}),
        ] {
            assert_eq!(
                resolved_create_note(&spoofed, activity_uri, actor_uri, object_uri)
                    .expect_err("spoofed Notes must fail permanently")
                    .disposition,
                FailureDisposition::Permanent
            );
        }
    }

    #[test]
    fn remote_note_fetch_failures_distinguish_retryable_and_permanent_errors() {
        assert_eq!(
            remote_note_fetch_failure(&RemoteFetchError::UnexpectedStatus(
                StatusCode::SERVICE_UNAVAILABLE
            ))
            .disposition,
            FailureDisposition::Retry
        );
        assert_eq!(
            remote_note_fetch_failure(&RemoteFetchError::UnexpectedStatus(StatusCode::GONE))
                .disposition,
            FailureDisposition::Permanent
        );
        assert_eq!(
            remote_note_fetch_failure(&RemoteFetchError::BlockedAddress(
                "127.0.0.1".parse().expect("loopback address")
            ))
            .disposition,
            FailureDisposition::Permanent
        );
    }

    #[test]
    fn remote_nested_announce_document_dereferences_cross_author_target() {
        let announce_uri = "https://remote.example/activities/boost";
        let note_uri = "https://remote.example/users/alice/statuses/1";
        let document = json!({
            "id": announce_uri,
            "type": "Announce",
            "actor": "https://remote.example/users/bob",
            "object": {
                "id": note_uri,
                "type": "Note",
                "attributedTo": "https://remote.example/users/alice",
                "content": "hello"
            },
            "to": ["https://www.w3.org/ns/activitystreams#Public"]
        });
        let RemoteAnnounceTarget::Announce {
            activity_uri,
            actor_uri,
            object_uri,
            embedded_note,
            to,
            ..
        } = remote_announce_document(&document, announce_uri).expect("valid nested Announce")
        else {
            panic!("expected nested Announce");
        };
        assert_eq!(activity_uri, announce_uri);
        assert_eq!(actor_uri, "https://remote.example/users/bob");
        assert_eq!(object_uri, note_uri);
        assert!(embedded_note.is_none());
        assert_eq!(to, ["https://www.w3.org/ns/activitystreams#Public"]);
    }

    #[test]
    fn delivery_failure_classification_retries_transient_statuses() {
        assert_eq!(
            delivery_failure(
                &RemoteFetchError::UnexpectedStatus(StatusCode::BAD_GATEWAY),
                false,
            )
            .disposition,
            FailureDisposition::Retry
        );
        assert_eq!(
            delivery_failure(
                &RemoteFetchError::UnexpectedStatus(StatusCode::TOO_MANY_REQUESTS),
                false
            )
            .disposition,
            FailureDisposition::Retry
        );
        assert_eq!(
            delivery_failure(
                &RemoteFetchError::UnexpectedStatus(StatusCode::UNAUTHORIZED),
                false
            )
            .disposition,
            FailureDisposition::Retry
        );
        assert_eq!(
            delivery_failure(
                &RemoteFetchError::UnexpectedStatus(StatusCode::UNAUTHORIZED),
                true,
            )
            .disposition,
            FailureDisposition::Permanent
        );
        assert_eq!(
            delivery_failure(
                &RemoteFetchError::UnexpectedStatus(StatusCode::NOT_IMPLEMENTED),
                false
            )
            .disposition,
            FailureDisposition::Permanent
        );
        assert_eq!(
            delivery_failure(&RemoteFetchError::Signing, false).disposition,
            FailureDisposition::Permanent
        );
        assert_eq!(
            delivery_failure(&RemoteFetchError::DomainBudgetExceeded, false).disposition,
            FailureDisposition::Retry
        );
    }

    #[test]
    fn remote_media_failure_classification_preserves_retryable_fetches() {
        assert_eq!(
            remote_media_fetch_failure(&RemoteFetchError::UnexpectedStatus(
                StatusCode::BAD_GATEWAY
            ))
            .disposition,
            FailureDisposition::Retry
        );
        assert_eq!(
            remote_media_fetch_failure(&RemoteFetchError::UnexpectedStatus(
                StatusCode::UNAUTHORIZED
            ))
            .disposition,
            FailureDisposition::Retry
        );
        assert_eq!(
            remote_media_fetch_failure(&RemoteFetchError::UnexpectedStatus(
                StatusCode::NOT_IMPLEMENTED
            ))
            .disposition,
            FailureDisposition::Permanent
        );
        assert_eq!(
            remote_media_fetch_failure(&RemoteFetchError::UnexpectedStatus(StatusCode::NOT_FOUND))
                .disposition,
            FailureDisposition::Permanent
        );
        assert_eq!(
            remote_media_fetch_failure(&RemoteFetchError::BodyTooLarge).disposition,
            FailureDisposition::Permanent
        );
        assert_eq!(
            remote_media_fetch_failure(&RemoteFetchError::DomainBudgetExceeded).disposition,
            FailureDisposition::Retry
        );
    }
}
