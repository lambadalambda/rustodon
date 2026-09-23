pub mod local_uploads;
mod profile_media;

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
    ACTIVITYPUB_NOTE_RESOLVE_JOB_KIND, ACTIVITYPUB_PROFILE_MEDIA_CLEANUP_JOB_KIND,
    ACTIVITYPUB_PROFILE_MEDIA_FETCH_JOB_KIND, ACTIVITYPUB_STATUS_DISTRIBUTION_JOB_KIND,
    ACTIVITYPUB_THREAD_RESOLVE_JOB_KIND, ClaimedJob, JobError, JobSpec,
    LOCAL_MEDIA_CLEANUP_JOB_KIND, Lane, MASTODON_ACCOUNT_PURGE_JOB_KIND,
    MASTODON_DOMAIN_BLOCK_JOB_KIND, MASTODON_DOMAIN_PURGE_JOB_KIND,
    MASTODON_POLL_EXPIRATION_EFFECT_KIND, MASTODON_POLL_EXPIRATION_JOB_KIND,
    MASTODON_POLL_EXPIRATION_RECONCILE_JOB_KIND, NOTIFICATION_CLEANUP_JOB_KIND,
    NOTIFICATION_CREATE_JOB_KIND, NOTIFICATION_UNFILTER_JOB_KIND, PollExpirationStartupClaim,
    Queue, WorkerHeartbeat, flush_stream_events_in, poll_expiration_effect_key,
    poll_expiration_generation, record_outbox_once_in, record_stream_event_in,
    validate_dispatched_poll_expiration_effect,
};
use crate::mail::MailRuntime;
use crate::mastodon::activitypub_inbox::{
    InboxActivity, InboxJob, parse_activity, parse_job_arguments, validate_note_object,
};
use crate::mastodon::{
    Account, AccountPurgeOutcome, HttpSignatureSigner, NotificationActivity, NotificationCreate,
    RemoteFollowOutcome, RemotePollVoteOutcome, RemoteUndoReferenceKind, Repository,
    STATUS_NOTIFICATION_JOB_KIND, StatusVisibility, WriteError, WriteRepository, activitypub,
    equals_or_includes,
};
use crate::paperclip::{
    PaperclipAttachment, PaperclipMetadata, PaperclipRoot, parse_paperclip_path,
    prepare_custom_emoji, prepare_rich_media_attachment, write_prepared_custom_emoji,
    write_prepared_media,
};
use crate::remote::{
    RemoteAccountResolver, RemoteFetchError, RemoteFetchLimits, RemoteFetcher, RemoteMediaFetcher,
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
const NOTIFICATION_CLEANUP_BATCH_SIZE: i64 = 1_000;

fn quote_reference(primary: Option<&str>, fallback: Option<&str>) -> Option<String> {
    primary
        .filter(|value| !crate::paperclip::rails_blank(value))
        .or_else(|| fallback.filter(|value| !crate::paperclip::rails_blank(value)))
        .map(ToOwned::to_owned)
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RemoteQuoteFetchReferences {
    target_uri: String,
    approval_uri: Option<String>,
}

fn remote_uri_reference(value: Option<&Value>) -> Option<&str> {
    let value = match value? {
        Value::Array(values) => values.first()?,
        value => value,
    };
    value
        .as_str()
        .or_else(|| value.as_object()?.get("id")?.as_str())
        .filter(|uri| {
            Url::parse(uri)
                .ok()
                .is_some_and(|url| matches!(url.scheme(), "http" | "https") && url.host().is_some())
        })
}

fn remote_note_quote_fetch_references(object: &Value) -> Option<RemoteQuoteFetchReferences> {
    let object = object.as_object()?;
    let quote = ["quote", "_misskey_quote", "quoteUrl", "quoteUri"]
        .into_iter()
        .find_map(|field| object.get(field))?;
    if quote
        .as_object()
        .is_some_and(|quote| equals_or_includes(quote.get("type"), "Tombstone"))
    {
        return None;
    }
    let target_uri = remote_uri_reference(Some(quote))?;
    let approval_uri = remote_uri_reference(object.get("quoteAuthorization")).map(str::to_owned);
    Some(RemoteQuoteFetchReferences {
        target_uri: target_uri.to_owned(),
        approval_uri,
    })
}

fn quote_request_instrument_target_uri(
    instrument: &Value,
    actor_uri: &str,
    instrument_uri: &str,
) -> Result<String, HandlerFailure> {
    let object = instrument.as_object().ok_or_else(|| {
        HandlerFailure::permanent("QuoteRequest instrument is not an embedded object")
    })?;
    if !equals_or_includes(object.get("type"), "Note")
        && !equals_or_includes(object.get("type"), "Question")
    {
        return Err(HandlerFailure::permanent(
            "QuoteRequest instrument is not a Note or Question",
        ));
    }
    if remote_uri_value(object.get("id")) != Some(instrument_uri) {
        return Err(HandlerFailure::permanent(
            "QuoteRequest instrument ID does not match the requested URI",
        ));
    }
    validate_note_object(actor_uri, object)
        .map_err(|_| HandlerFailure::permanent("QuoteRequest instrument is not a valid Note"))?;
    remote_note_quote_fetch_references(instrument)
        .map(|references| references.target_uri)
        .ok_or_else(|| {
            HandlerFailure::permanent("QuoteRequest instrument has no valid quote target")
        })
}

async fn validate_quote_request_instrument(
    writer: &WriteRepository,
    instrument: &Value,
    actor_uri: &str,
    instrument_uri: &str,
    target_status_id: i64,
    origin: &str,
) -> Result<(), HandlerFailure> {
    let target_uri = quote_request_instrument_target_uri(instrument, actor_uri, instrument_uri)?;
    let target_matches = writer
        .remote_quote_target_matches_status(target_status_id, &target_uri, origin)
        .await
        .map_err(|error| {
            remote_note_write_failure(&error, "QuoteRequest target binding lookup failed")
        })?;
    if !target_matches {
        return Err(HandlerFailure::permanent(
            "QuoteRequest instrument quote target does not match the requested status",
        ));
    }
    Ok(())
}

fn quote_decision_allows_follow_fallback(
    accepted: bool,
    request_actor_uri: Option<&str>,
    object_uri: Option<&str>,
    instrument_uri: Option<&str>,
) -> bool {
    accepted && request_actor_uri.is_none() && object_uri.is_none() && instrument_uri.is_none()
}

async fn activitypub_quote_parts(
    repository: &Repository,
    config: &ActivityPubDeliveryConfig,
    status_id: i64,
) -> Result<(Option<String>, Option<String>, Option<String>), HandlerFailure> {
    let Some(target) = repository
        .activitypub_quote_target_for_delivery(status_id)
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
        quote_reference(target.url.as_deref(), target.uri.as_deref())
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
            quote_reference(target.url.as_deref(), None)
        }
    });
    let quote_authorization = if target.accepted {
        quoted_identifier.as_ref().and_then(|_| {
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
        })
    } else {
        None
    };
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

/// Operator-only recovery of an existing remote account using its DB canonical ID.
/// Uses the signed, identity/WebFinger/SSRF-validated resolver and the normal image
/// persistence/outbox path. No relationships, local suspensions or keys are replaced.
///
/// # Errors
/// Fails closed on missing/local accounts, policy denial, invalid identity, absent
/// instance signing keys, transport failures or persistence failures.
pub async fn refresh_remote_account(
    pool: PgPool,
    config: &ActivityPubDeliveryConfig,
    account_id: i64,
) -> Result<(), HandlerFailure> {
    let repository = Repository::from_pool(pool.clone());
    let account = repository
        .account(account_id)
        .await
        .map_err(|_| HandlerFailure::retry("remote refresh account lookup failed"))?
        .ok_or_else(|| HandlerFailure::permanent("remote refresh account does not exist"))?;
    let domain = account
        .domain
        .as_deref()
        .filter(|domain| !domain.is_empty())
        .ok_or_else(|| HandlerFailure::permanent("remote refresh requires a remote account"))?;
    if !repository
        .remote_domain_allowed(domain, config.limited_federation)
        .await
        .map_err(|_| HandlerFailure::retry("remote refresh policy lookup failed"))?
    {
        return Err(HandlerFailure::permanent(
            "remote refresh domain is not allowed",
        ));
    }
    let actor_url = Url::parse(&account.uri).map_err(|_| {
        HandlerFailure::permanent("remote refresh account has no canonical actor ID")
    })?;
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
    let key_id = format!(
        "{}#main-key",
        activitypub::actor_url(&config.origin, &instance)
    );
    let signer = HttpSignatureSigner {
        key_id: &key_id,
        private_key_pem: private_key.as_str(),
    };
    let fetcher = RemoteFetcher::new(RemoteFetchLimits::default());
    #[cfg(feature = "test-support")]
    let fetcher = fetcher.with_test_endpoint(config.remote_fetch_endpoint);
    let actor = RemoteAccountResolver::new(fetcher)
        .resolve_actor_uri_with_signer(&actor_url, Some(&signer))
        .await
        .map_err(|error| remote_thread_fetch_failure(&error))?;
    WriteRepository::from_pool(pool)
        .refresh_remote_actor(
            account_id,
            &account.username,
            domain,
            config.limited_federation,
            &actor,
        )
        .await
        .map_err(|_| HandlerFailure::retry("remote refresh persistence failed"))?;
    Ok(())
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
    StartupReconciliationFailed,
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
            Self::StartupReconciliationFailed => {
                formatter.write_str("startup poll expiration reconciliation failed")
            }
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
            | Self::StartupReconciliationFailed
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

async fn transition_handler_failure(
    queue: &Queue,
    job: &ClaimedJob,
    failure: &HandlerFailure,
) -> Result<(), JobError> {
    if failure.disposition == FailureDisposition::Permanent {
        queue.dead_letter(job, &failure.message).await?;
    } else {
        queue
            .retry(
                job,
                Utc::now() + retry_delay(job.id, job.attempt),
                &failure.message,
            )
            .await?;
    }
    Ok(())
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
                    let renewal = self.queue.renew(
                        job.id,
                        &job.lease_owner,
                        job.generation,
                        lease_duration,
                    );
                    tokio::pin!(renewal);
                    let renewed = tokio::select! {
                        biased;
                        result = &mut future => break Some(result?),
                        renewed = &mut renewal => renewed?,
                    };
                    if !renewed {
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
                // durable job reclaimable. Reconciliation publishes its exact success watermark
                // in the same fenced transaction that removes the claimed job.
                if job.kind == MASTODON_POLL_EXPIRATION_RECONCILE_JOB_KIND {
                    self.queue
                        .complete_poll_expiration_reconciliation_success(&job)
                        .await?;
                } else {
                    self.queue
                        .complete(job.id, &job.lease_owner, job.generation)
                        .await?;
                }
            }
            Some(Err(failure)) => {
                transition_handler_failure(&self.queue, &job, &failure).await?;
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StatusUpdateKind {
    Status,
    Poll,
    Quote,
    InteractionPolicy,
    StatusRepair,
    PollRepair,
}

impl StatusUpdateKind {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "status" => Some(Self::Status),
            "poll" => Some(Self::Poll),
            "quote" => Some(Self::Quote),
            "interaction_policy" => Some(Self::InteractionPolicy),
            "status_repair" => Some(Self::StatusRepair),
            "poll_repair" => Some(Self::PollRepair),
            _ => None,
        }
    }

    const fn is_repair(self) -> bool {
        matches!(self, Self::StatusRepair | Self::PollRepair)
    }

    const fn has_poll_reach(self) -> bool {
        matches!(self, Self::Poll | Self::PollRepair)
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::Poll => "poll",
            Self::Quote => "quote",
            Self::InteractionPolicy => "interaction_policy",
            Self::StatusRepair => "status_repair",
            Self::PollRepair => "poll_repair",
        }
    }
}

fn status_snapshot_repair_activity_id(
    object_uri: &str,
    edited_at: NaiveDateTime,
    poll_updated_at: Option<NaiveDateTime>,
) -> String {
    let edited_at_micros = edited_at.and_utc().timestamp_micros();
    let poll_version = poll_updated_at.map_or_else(
        || "none".to_owned(),
        |value| value.and_utc().timestamp_micros().to_string(),
    );
    format!("{object_uri}#updates/repair/{edited_at_micros}/{poll_version}")
}

fn status_update_versions(
    current_edited_at: NaiveDateTime,
    current_poll_updated_at: Option<NaiveDateTime>,
    requested_edited_at: Option<NaiveDateTime>,
    requested_poll_updated_at: Option<NaiveDateTime>,
    update_kind: StatusUpdateKind,
    strict_snapshot: bool,
) -> Option<(NaiveDateTime, Option<NaiveDateTime>, NaiveDateTime)> {
    if update_kind == StatusUpdateKind::StatusRepair && current_poll_updated_at.is_some() {
        return None;
    }
    let edited_at = match requested_edited_at {
        Some(requested) if requested == current_edited_at => requested,
        Some(_) => return None,
        None if strict_snapshot => return None,
        None => current_edited_at,
    };
    let poll_updated_at = match (requested_poll_updated_at, current_poll_updated_at) {
        (Some(requested), Some(current)) if requested == current => Some(requested),
        (Some(_), Some(current)) if update_kind == StatusUpdateKind::Poll => Some(current),
        (None, Some(_)) if strict_snapshot => return None,
        (None, Some(current)) => Some(current),
        (None, None) => None,
        _ => return None,
    };
    let update_version = match update_kind {
        StatusUpdateKind::Status
        | StatusUpdateKind::InteractionPolicy
        | StatusUpdateKind::StatusRepair => edited_at,
        StatusUpdateKind::Poll => poll_updated_at?,
        StatusUpdateKind::PollRepair => edited_at.max(poll_updated_at?),
        StatusUpdateKind::Quote => return None,
    };
    Some((edited_at, poll_updated_at, update_version))
}

fn quote_revision_is_current(
    current_quote_updated_at: Option<NaiveDateTime>,
    requested_quote_updated_at_micros: Option<i64>,
) -> bool {
    current_quote_updated_at.map(|value| value.and_utc().timestamp_micros())
        == requested_quote_updated_at_micros
}

fn quote_update_versions(
    current_edited_at: NaiveDateTime,
    current_poll_updated_at: Option<NaiveDateTime>,
    current_quote_updated_at: Option<NaiveDateTime>,
    requested_edited_at: Option<NaiveDateTime>,
    requested_poll_updated_at: Option<NaiveDateTime>,
    requested_quote_updated_at_micros: Option<i64>,
    requested_update_version: Option<NaiveDateTime>,
) -> Option<(NaiveDateTime, Option<NaiveDateTime>, NaiveDateTime)> {
    (requested_edited_at == Some(current_edited_at)
        && requested_poll_updated_at == current_poll_updated_at
        && quote_revision_is_current(current_quote_updated_at, requested_quote_updated_at_micros))
    .then_some((
        current_edited_at,
        current_poll_updated_at,
        requested_update_version?,
    ))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StatusUpdateVersionDecision {
    Deliver(NaiveDateTime, Option<NaiveDateTime>, NaiveDateTime),
    Repair,
    Stale,
}

fn status_update_version_decision(
    current_edited_at: NaiveDateTime,
    current_poll_updated_at: Option<NaiveDateTime>,
    requested_edited_at: Option<NaiveDateTime>,
    requested_poll_updated_at: Option<NaiveDateTime>,
    update_kind: StatusUpdateKind,
    strict_snapshot: bool,
) -> StatusUpdateVersionDecision {
    if strict_snapshot
        && update_kind.is_repair()
        && (requested_edited_at != Some(current_edited_at)
            || requested_poll_updated_at != current_poll_updated_at)
    {
        return StatusUpdateVersionDecision::Repair;
    }
    match status_update_versions(
        current_edited_at,
        current_poll_updated_at,
        requested_edited_at,
        requested_poll_updated_at,
        update_kind,
        strict_snapshot,
    ) {
        Some((edited_at, poll_updated_at, update_version)) => {
            StatusUpdateVersionDecision::Deliver(edited_at, poll_updated_at, update_version)
        }
        None if strict_snapshot => StatusUpdateVersionDecision::Repair,
        None => StatusUpdateVersionDecision::Stale,
    }
}

async fn queue_status_snapshot_repair(
    pool: &PgPool,
    status_id: i64,
    poll_id: Option<i64>,
    edited_at: NaiveDateTime,
    poll_updated_at: Option<NaiveDateTime>,
) -> Result<(), HandlerFailure> {
    let edited_at_micros = edited_at.and_utc().timestamp_micros();
    let (update_kind, update_version, poll_updated_at_micros) = match poll_updated_at {
        Some(poll_updated_at) => (
            StatusUpdateKind::PollRepair,
            edited_at.max(poll_updated_at),
            Some(poll_updated_at.and_utc().timestamp_micros()),
        ),
        None => (StatusUpdateKind::StatusRepair, edited_at, None),
    };
    let update_version_micros = update_version.and_utc().timestamp_micros();
    let repair = JobSpec::new(
        Lane::Push,
        ACTIVITYPUB_STATUS_DISTRIBUTION_JOB_KIND,
        json!({
            "status_id": status_id,
            "activity_type": "Update",
            "update_kind": update_kind.as_str(),
            "update_version_micros": update_version_micros,
            "edited_at_micros": edited_at_micros,
            "poll_updated_at_micros": poll_updated_at_micros,
        }),
    )
    .logical_key(format!(
        "activitypub:status:{status_id}:snapshot-repair:{}:{edited_at_micros}:{poll_updated_at_micros:?}",
        poll_id.unwrap_or_default()
    ));
    let mut transaction = pool
        .begin()
        .await
        .map_err(|_| HandlerFailure::retry("poll snapshot repair transaction failed"))?;
    record_outbox_once_in(&mut transaction, &repair)
        .await
        .map_err(|_| HandlerFailure::retry("poll snapshot repair outbox write failed"))?;
    transaction
        .commit()
        .await
        .map_err(|_| HandlerFailure::retry("poll snapshot repair commit failed"))
}

struct QuoteRequestDistribution {
    quote_id: i64,
    request_uri: String,
    quoted_status_id: i64,
    quoted_status_uri: String,
    quoted_status_url: String,
    quoted_account_id: i64,
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn distribute_status(
    pool: PgPool,
    config: &ActivityPubDeliveryConfig,
    status_id: i64,
    activity_type: &str,
    edited_at_micros: Option<i64>,
    poll_updated_at_micros: Option<i64>,
    quote_updated_at_micros: Option<i64>,
    update_kind: Option<&str>,
    update_version_micros: Option<i64>,
    explicit_recipient_ids: &[i64],
    quote_request: Option<&QuoteRequestDistribution>,
) -> Result<(), HandlerFailure> {
    let is_quote_request = activity_type == "QuoteRequest";
    let is_delete = match activity_type {
        "Create" | "Update" | "QuoteRequest" => false,
        "Delete" => true,
        _ => {
            return Err(HandlerFailure::permanent(
                "status distribution activity type is unsupported",
            ));
        }
    };
    if is_quote_request != quote_request.is_some() {
        return Err(HandlerFailure::permanent(
            "status quote-request distribution arguments are invalid",
        ));
    }
    let requested_edited_at = edited_at_micros
        .map(|value| {
            DateTime::<Utc>::from_timestamp_micros(value)
                .map(|timestamp| timestamp.naive_utc())
                .ok_or_else(|| HandlerFailure::permanent("status edit timestamp is invalid"))
        })
        .transpose()?;
    let requested_poll_updated_at = poll_updated_at_micros
        .map(|value| {
            DateTime::<Utc>::from_timestamp_micros(value)
                .map(|timestamp| timestamp.naive_utc())
                .ok_or_else(|| HandlerFailure::permanent("poll update timestamp is invalid"))
        })
        .transpose()?;
    let requested_update_version = update_version_micros
        .map(|value| {
            DateTime::<Utc>::from_timestamp_micros(value)
                .map(|timestamp| timestamp.naive_utc())
                .ok_or_else(|| HandlerFailure::permanent("status update version is invalid"))
        })
        .transpose()?;
    let explicit_update_kind = update_kind.is_some();
    let resolved_update_kind = match update_kind {
        Some(value) => StatusUpdateKind::parse(value).ok_or_else(|| {
            HandlerFailure::permanent("status distribution update kind is unsupported")
        })?,
        None if requested_poll_updated_at.is_some() && requested_edited_at.is_none() => {
            StatusUpdateKind::Poll
        }
        None if requested_poll_updated_at.is_some()
            && update_version_micros == poll_updated_at_micros =>
        {
            StatusUpdateKind::Poll
        }
        None if requested_poll_updated_at.is_some()
            && update_version_micros == edited_at_micros =>
        {
            StatusUpdateKind::Status
        }
        None if requested_poll_updated_at.is_some() => StatusUpdateKind::PollRepair,
        None => StatusUpdateKind::Status,
    };
    let poll_update = resolved_update_kind.has_poll_reach();
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
    if status.reblog_of_id.is_some() && is_quote_request {
        return Err(HandlerFailure::permanent(
            "quote-request instrument cannot be a reblog",
        ));
    }
    let current_edited_at = if resolved_update_kind == StatusUpdateKind::InteractionPolicy {
        status.updated_at
    } else {
        status.edited_at.unwrap_or(status.updated_at)
    };
    let (requested_edited_at, requested_poll_updated_at, selected_update_version) = if activity_type
        == "Update"
    {
        let current_poll_updated_at = if let Some(poll_id) = status.poll_id {
            sqlx::query_scalar::<_, NaiveDateTime>("SELECT updated_at FROM polls WHERE id = $1")
                .bind(poll_id)
                .fetch_optional(&pool)
                .await
                .map_err(|_| HandlerFailure::retry("poll update fence lookup failed"))?
        } else {
            None
        };
        let version_decision = if resolved_update_kind == StatusUpdateKind::Quote {
            let current_quote_updated_at = sqlx::query_scalar::<_, NaiveDateTime>(
                "SELECT updated_at FROM quotes WHERE status_id = $1 ORDER BY id LIMIT 1",
            )
            .bind(status_id)
            .fetch_optional(&pool)
            .await
            .map_err(|_| HandlerFailure::retry("quote update fence lookup failed"))?;
            quote_update_versions(
                current_edited_at,
                current_poll_updated_at,
                current_quote_updated_at,
                requested_edited_at,
                requested_poll_updated_at,
                quote_updated_at_micros,
                requested_update_version,
            )
            .map_or(
                StatusUpdateVersionDecision::Stale,
                |(edited_at, poll_updated_at, update_version)| {
                    StatusUpdateVersionDecision::Deliver(edited_at, poll_updated_at, update_version)
                },
            )
        } else {
            status_update_version_decision(
                current_edited_at,
                current_poll_updated_at,
                requested_edited_at,
                requested_poll_updated_at,
                resolved_update_kind,
                explicit_update_kind || resolved_update_kind.is_repair(),
            )
        };
        match version_decision {
            StatusUpdateVersionDecision::Deliver(edited_at, poll_updated_at, update_version) => {
                (Some(edited_at), poll_updated_at, Some(update_version))
            }
            StatusUpdateVersionDecision::Repair if resolved_update_kind.is_repair() => {
                let Some((edited_at, poll_updated_at, update_version)) = status_update_versions(
                    current_edited_at,
                    current_poll_updated_at,
                    Some(current_edited_at),
                    current_poll_updated_at,
                    resolved_update_kind,
                    true,
                ) else {
                    queue_status_snapshot_repair(
                        &pool,
                        status_id,
                        status.poll_id,
                        current_edited_at,
                        current_poll_updated_at,
                    )
                    .await?;
                    return Ok(());
                };
                (Some(edited_at), poll_updated_at, Some(update_version))
            }
            StatusUpdateVersionDecision::Repair => {
                queue_status_snapshot_repair(
                    &pool,
                    status_id,
                    status.poll_id,
                    current_edited_at,
                    current_poll_updated_at,
                )
                .await?;
                return Ok(());
            }
            StatusUpdateVersionDecision::Stale => return Ok(()),
        }
    } else {
        (requested_edited_at, requested_poll_updated_at, None)
    };
    let selected_update_version_micros = selected_update_version
        .map(|value| value.and_utc().timestamp_micros())
        .or(update_version_micros);
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
                let mut object = activitypub::note(
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
                );
                if let Some(poll_id) = target_status.poll_id {
                    let loaded_poll = repository
                        .poll(poll_id)
                        .await
                        .map_err(|_| HandlerFailure::retry("private boost poll lookup failed"))?
                        .ok_or_else(|| {
                            HandlerFailure::permanent("private boost poll is missing")
                        })?;
                    object = activitypub::question(object, &loaded_poll, Utc::now().naive_utc());
                }
                object
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
            if let Some(quote_request) = quote_request {
                (
                    Some(quote_request.quoted_status_url.clone()),
                    Some(quote_request.quoted_status_uri.clone()),
                    None,
                )
            } else {
                activitypub_quote_parts(&repository, config, status_id).await?
            };
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
        if let Some(poll_id) = status.poll_id {
            let loaded_poll = repository
                .poll(poll_id)
                .await
                .map_err(|_| HandlerFailure::retry("status poll lookup failed"))?
                .ok_or_else(|| HandlerFailure::permanent("status poll is missing"))?;
            object = activitypub::question(object, &loaded_poll, Utc::now().naive_utc());
        }
        if let Some(quote_request) = quote_request {
            activitypub::quote_request_with_uris(
                &quote_request.request_uri,
                &activitypub::actor_url(&config.origin, &account),
                &quote_request.quoted_status_uri,
                object,
            )
        } else if activity_type == "Update" {
            let object_uri = object["id"]
                .as_str()
                .ok_or_else(|| HandlerFailure::permanent("status Note has no ID"))?
                .to_owned();
            let update_version = selected_update_version
                .ok_or_else(|| HandlerFailure::permanent("status Update has no version"))?;
            if !poll_update {
                object["updated"] = json!(activitypub::timestamp(update_version));
            }
            let update_uri = if resolved_update_kind.is_repair() {
                status_snapshot_repair_activity_id(
                    &object_uri,
                    requested_edited_at
                        .ok_or_else(|| HandlerFailure::permanent("repair has no status version"))?,
                    requested_poll_updated_at,
                )
            } else {
                activitypub::update_activity_id(&object_uri, update_version)
            };
            activitypub::update_with_uris(
                &update_uri,
                &activitypub::actor_url(&config.origin, &account),
                update_version,
                object,
            )
        } else {
            activitypub::create(&config.origin, &account, &status, object)
        }
    };
    let include_unsafe_reach = is_delete;
    let recipient_ids = if let Some(quote_request) = quote_request {
        BTreeSet::from([quote_request.quoted_account_id])
    } else {
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
        if poll_update {
            recipient_ids.extend(
                repository
                    .activitypub_poll_voter_account_ids(status_id)
                    .await
                    .map_err(|_| HandlerFailure::retry("poll voter reach lookup failed"))?,
            );
        }
        recipient_ids.extend(mentioned_recipient_ids);
        recipient_ids
    };
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
        let inbox_url = if is_quote_request || follower.shared_inbox_url.is_empty() {
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
    if !is_quote_request && status.visibility == StatusVisibility::Public {
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
    if is_quote_request {
        let quoted_status_id = sqlx::query_scalar::<_, Option<i64>>(
            "SELECT quoted_status_id FROM quotes
              WHERE id = $1 AND status_id = $2 AND quoted_status_id = $3
                AND state = 0 AND activity_uri = $4
              FOR UPDATE",
        )
        .bind(
            quote_request
                .expect("QuoteRequest has distribution metadata")
                .quote_id,
        )
        .bind(status_id)
        .bind(
            quote_request
                .expect("QuoteRequest has distribution metadata")
                .quoted_status_id,
        )
        .bind(activity_id)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|_| HandlerFailure::retry("quote-request lifecycle fence failed"))?
        .flatten();
        let relationship_is_live = if let Some(quoted_status_id) = quoted_status_id {
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS (
                   SELECT 1 FROM statuses quoting
                   JOIN statuses quoted ON quoted.id = $2
                  WHERE quoting.id = $1 AND quoting.local IS TRUE
                    AND quoting.deleted_at IS NULL AND quoted.deleted_at IS NULL)",
            )
            .bind(status_id)
            .bind(quoted_status_id)
            .fetch_one(&mut *transaction)
            .await
            .map_err(|_| HandlerFailure::retry("quote-request status fence failed"))?
        } else {
            false
        };
        if !relationship_is_live {
            transaction
                .commit()
                .await
                .map_err(|_| HandlerFailure::retry("quote-request fence commit failed"))?;
            return Ok(());
        }
    }
    for (inbox_url, remote_domain) in inboxes {
        let logical_key = match activity_type {
            "Create" => delivery_logical_key(status_id, &inbox_url),
            "Update" => update_delivery_logical_key(
                status_id,
                activity_id,
                selected_update_version_micros.expect("Update activity has an edit version"),
                &inbox_url,
            ),
            "Delete" => delete_delivery_logical_key(status_id, &inbox_url),
            "QuoteRequest" => {
                format!("activitypub:quote-request:{activity_id}:{inbox_url}")
            }
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
                "update_kind": (activity_type == "Update").then_some(resolved_update_kind.as_str()),
                "update_version_micros": selected_update_version_micros,
                "edited_at_micros": requested_edited_at.map(|value| value.and_utc().timestamp_micros()),
                "poll_updated_at_micros": requested_poll_updated_at.map(|value| value.and_utc().timestamp_micros()),
                "quote_updated_at_micros": quote_updated_at_micros,
                "quote_delivery_kind": is_quote_request.then_some("request"),
                "quote_request_uri": quote_request.map(|request| request.request_uri.as_str()),
                "quote_id": quote_request.map(|request| request.quote_id),
                "quoting_status_id": is_quote_request.then_some(status_id),
                "quoted_status_id": quote_request.map(|request| request.quoted_status_id),
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

#[allow(clippy::too_many_lines)]
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
            let owned: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM rustodon.local_uploads WHERE media_id = $1)",
            )
            .bind(media_id)
            .fetch_one(&mut *transaction)
            .await?;
            if owned && action == "rollback_create" {
                return Ok(());
            }
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
        Some("poll") => NotificationActivity::Poll { id: activity_id },
        Some("update") => NotificationActivity::Update { id: activity_id },
        Some("quoted_update") => NotificationActivity::QuotedUpdate { id: activity_id },
        Some("quote") => NotificationActivity::Quote { id: activity_id },
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

fn update_delivery_is_current(
    activity_id: Option<&str>,
    object_uri: &str,
    version: NaiveDateTime,
    requested_version_micros: Option<i64>,
) -> bool {
    if requested_version_micros
        .is_some_and(|requested| requested != version.and_utc().timestamp_micros())
    {
        return false;
    }
    let expected_activity_id = activitypub::update_activity_id(object_uri, version);
    // Already-persisted deliveries keep their original body/ID across an upgrade.
    // Their microsecond metadata still fences superseded versions.
    let legacy_activity_id = format!("{object_uri}#updates/{}", version.and_utc().timestamp());
    activity_id == Some(expected_activity_id.as_str())
        || activity_id == Some(legacy_activity_id.as_str())
}

#[allow(clippy::too_many_arguments)]
fn status_update_delivery_is_current(
    update_kind: StatusUpdateKind,
    activity_id: Option<&str>,
    object_uri: &str,
    current_edited_at: NaiveDateTime,
    current_poll_updated_at: Option<NaiveDateTime>,
    requested_edited_at_micros: Option<i64>,
    requested_poll_updated_at_micros: Option<i64>,
    requested_update_version_micros: Option<i64>,
) -> bool {
    if requested_edited_at_micros != Some(current_edited_at.and_utc().timestamp_micros())
        || requested_poll_updated_at_micros
            != current_poll_updated_at.map(|value| value.and_utc().timestamp_micros())
    {
        return false;
    }
    let expected_update_version = match update_kind {
        StatusUpdateKind::Status
        | StatusUpdateKind::InteractionPolicy
        | StatusUpdateKind::StatusRepair => current_edited_at,
        StatusUpdateKind::Poll => {
            let Some(poll_updated_at) = current_poll_updated_at else {
                return false;
            };
            poll_updated_at
        }
        StatusUpdateKind::PollRepair => {
            let Some(poll_updated_at) = current_poll_updated_at else {
                return false;
            };
            current_edited_at.max(poll_updated_at)
        }
        StatusUpdateKind::Quote => {
            let Some(version_micros) = requested_update_version_micros else {
                return false;
            };
            let Some(version) = DateTime::<Utc>::from_timestamp_micros(version_micros) else {
                return false;
            };
            version.naive_utc()
        }
    };
    if requested_update_version_micros != Some(expected_update_version.and_utc().timestamp_micros())
    {
        return false;
    }
    if update_kind.is_repair() {
        return activity_id
            == Some(
                status_snapshot_repair_activity_id(
                    object_uri,
                    current_edited_at,
                    current_poll_updated_at,
                )
                .as_str(),
            );
    }
    update_delivery_is_current(
        activity_id,
        object_uri,
        expected_update_version,
        requested_update_version_micros,
    )
}

fn complete_status_update_delivery_kind(
    update_kind: Option<&str>,
    has_current_poll: bool,
    requested_edited_at_micros: Option<i64>,
    requested_poll_updated_at_micros: Option<i64>,
    requested_update_version_micros: Option<i64>,
    published_version_micros: Option<i64>,
) -> Option<StatusUpdateKind> {
    let update_kind = StatusUpdateKind::parse(update_kind?)?;
    let edited_at_micros = requested_edited_at_micros?;
    if requested_poll_updated_at_micros.is_some() != has_current_poll
        || (update_kind.has_poll_reach() && !has_current_poll)
        || (update_kind == StatusUpdateKind::StatusRepair && has_current_poll)
    {
        return None;
    }
    let expected_update_version_micros = match update_kind {
        StatusUpdateKind::Status
        | StatusUpdateKind::InteractionPolicy
        | StatusUpdateKind::StatusRepair => edited_at_micros,
        StatusUpdateKind::Poll => requested_poll_updated_at_micros?,
        StatusUpdateKind::PollRepair => edited_at_micros.max(requested_poll_updated_at_micros?),
        StatusUpdateKind::Quote => requested_update_version_micros?,
    };
    if requested_update_version_micros != Some(expected_update_version_micros)
        || published_version_micros != Some(expected_update_version_micros)
    {
        return None;
    }
    Some(update_kind)
}

fn inferred_current_repair_delivery_kind(
    activity_id: Option<&str>,
    object_uri: &str,
    current_edited_at: NaiveDateTime,
    current_poll_updated_at: Option<NaiveDateTime>,
    published_version_micros: Option<i64>,
) -> Option<StatusUpdateKind> {
    let update_kind = if current_poll_updated_at.is_some() {
        StatusUpdateKind::PollRepair
    } else {
        StatusUpdateKind::StatusRepair
    };
    let expected_update_version = current_poll_updated_at
        .map_or(current_edited_at, |poll_updated_at| {
            current_edited_at.max(poll_updated_at)
        });
    let expected_activity_id =
        status_snapshot_repair_activity_id(object_uri, current_edited_at, current_poll_updated_at);
    (activity_id == Some(expected_activity_id.as_str())
        && published_version_micros == Some(expected_update_version.and_utc().timestamp_micros()))
    .then_some(update_kind)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum QuoteDeliveryKind {
    Request,
    Accept,
    Reject,
}

#[derive(Debug, Eq, PartialEq)]
struct QuoteDeliveryIdentity {
    kind: QuoteDeliveryKind,
    request_uri: String,
    quote_id: Option<i64>,
    quoting_status_id: Option<i64>,
    quoted_status_id: i64,
}

fn quote_delivery_identity(
    arguments: &Value,
    body: &Value,
) -> Result<Option<QuoteDeliveryIdentity>, ()> {
    let body_type = body.get("type").and_then(Value::as_str);
    let nested_type = body
        .get("object")
        .and_then(|object| object.get("type"))
        .and_then(Value::as_str);
    let body_kind = match (body_type, nested_type) {
        (Some("QuoteRequest"), _) => Some(QuoteDeliveryKind::Request),
        (Some("Accept"), Some("QuoteRequest")) => Some(QuoteDeliveryKind::Accept),
        (Some("Reject"), Some("QuoteRequest")) => Some(QuoteDeliveryKind::Reject),
        _ => None,
    };
    let metadata_kind = match arguments.get("quote_delivery_kind").and_then(Value::as_str) {
        Some("request") => Some(QuoteDeliveryKind::Request),
        Some("accept") => Some(QuoteDeliveryKind::Accept),
        Some("reject") => Some(QuoteDeliveryKind::Reject),
        Some(_) => return Err(()),
        None => None,
    };
    let Some(kind) = body_kind else {
        return if metadata_kind.is_some() {
            Err(())
        } else {
            Ok(None)
        };
    };
    if metadata_kind != Some(kind) {
        return Err(());
    }
    let request_uri = arguments
        .get("quote_request_uri")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or(())?;
    let body_request_uri = match kind {
        QuoteDeliveryKind::Request => body.get("id").and_then(Value::as_str),
        QuoteDeliveryKind::Accept | QuoteDeliveryKind::Reject => body
            .get("object")
            .and_then(|object| object.get("id"))
            .and_then(Value::as_str),
    };
    if body_request_uri != Some(request_uri)
        || (kind == QuoteDeliveryKind::Accept && body.get("result").is_none_or(Value::is_null))
        || (kind == QuoteDeliveryKind::Reject
            && body.get("result").is_some_and(|value| !value.is_null()))
    {
        return Err(());
    }
    let quote_id = arguments.get("quote_id").and_then(Value::as_i64);
    let quoting_status_id = arguments.get("quoting_status_id").and_then(Value::as_i64);
    let quoted_status_id = arguments
        .get("quoted_status_id")
        .and_then(Value::as_i64)
        .ok_or(())?;
    match kind {
        QuoteDeliveryKind::Request | QuoteDeliveryKind::Accept
            if quote_id.is_none() || quoting_status_id.is_none() =>
        {
            return Err(());
        }
        QuoteDeliveryKind::Reject if quote_id.is_some() != quoting_status_id.is_some() => {
            return Err(());
        }
        _ => {}
    }
    Ok(Some(QuoteDeliveryIdentity {
        kind,
        request_uri: request_uri.to_owned(),
        quote_id,
        quoting_status_id,
        quoted_status_id,
    }))
}

fn quote_body_uri(value: Option<&Value>) -> Option<&str> {
    remote_uri_value(value).filter(|value| !value.is_empty())
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn quote_delivery_is_current_and_locked(
    transaction: &mut Transaction<'_, Postgres>,
    identity: &QuoteDeliveryIdentity,
    body: &Value,
    source_account_id: i64,
    source_actor_uri: &str,
    inbox_url: &str,
    configured_remote_domain: Option<&str>,
    origin: &str,
) -> Result<bool, sqlx::Error> {
    if quote_body_uri(body.get("actor")) != Some(source_actor_uri) {
        return Ok(false);
    }
    let request = if identity.kind == QuoteDeliveryKind::Request {
        body
    } else {
        body.get("object").unwrap_or(&Value::Null)
    };
    let Some(request_actor_uri) = quote_body_uri(request.get("actor")) else {
        return Ok(false);
    };
    let Some(request_target_uri) = quote_body_uri(request.get("object")) else {
        return Ok(false);
    };
    let Some(request_instrument_uri) = quote_body_uri(request.get("instrument")) else {
        return Ok(false);
    };
    let mut status_ids = vec![identity.quoted_status_id];
    if let Some(quoting_status_id) = identity.quoting_status_id {
        status_ids.push(quoting_status_id);
    }
    status_ids.sort_unstable();
    status_ids.dedup();
    let mut account_ids = sqlx::query_scalar::<_, i64>(
        "SELECT DISTINCT account_id FROM statuses WHERE id = ANY($1::bigint[]) ORDER BY account_id",
    )
    .bind(&status_ids)
    .fetch_all(&mut **transaction)
    .await?;
    account_ids.push(source_account_id);
    let requester_id = sqlx::query_scalar::<_, i64>(
        "SELECT id FROM accounts WHERE uri = $1 AND domain IS NOT NULL ORDER BY id LIMIT 1",
    )
    .bind(request_actor_uri)
    .fetch_optional(&mut **transaction)
    .await?;
    if let Some(requester_id) = requester_id {
        account_ids.push(requester_id);
    }
    let peer_account_id = if identity.kind == QuoteDeliveryKind::Request {
        sqlx::query_scalar::<_, i64>("SELECT account_id FROM statuses WHERE id = $1")
            .bind(identity.quoted_status_id)
            .fetch_optional(&mut **transaction)
            .await?
    } else {
        requester_id
    };
    let Some(peer_account_id) = peer_account_id.filter(|id| *id != source_account_id) else {
        return Ok(false);
    };
    sqlx::query(
        "SELECT pg_advisory_xact_lock( \
           hashtextextended(LEAST($1, $2)::text || ':' || GREATEST($1, $2)::text, 0))",
    )
    .bind(source_account_id)
    .bind(peer_account_id)
    .execute(&mut **transaction)
    .await?;
    account_ids.sort_unstable();
    account_ids.dedup();
    sqlx::query_scalar::<_, i64>(
        "SELECT id FROM accounts WHERE id = ANY($1::bigint[]) ORDER BY id FOR SHARE",
    )
    .bind(&account_ids)
    .fetch_all(&mut **transaction)
    .await?;
    let locked_status_ids = sqlx::query_scalar::<_, i64>(
        "SELECT id FROM statuses WHERE id = ANY($1::bigint[]) ORDER BY id FOR SHARE",
    )
    .bind(&status_ids)
    .fetch_all(&mut **transaction)
    .await?;
    if locked_status_ids.len() != status_ids.len() {
        return Ok(false);
    }
    if sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS ( \
           SELECT 1 FROM blocks \
           WHERE (account_id = $1 AND target_account_id = $2) \
              OR (account_id = $2 AND target_account_id = $1))",
    )
    .bind(source_account_id)
    .bind(peer_account_id)
    .fetch_one(&mut **transaction)
    .await?
    {
        return Ok(false);
    }
    let origin = origin.trim_end_matches('/');
    match identity.kind {
        QuoteDeliveryKind::Request => {
            let Some(quote_id) = identity.quote_id else {
                return Ok(false);
            };
            let Some(quoting_status_id) = identity.quoting_status_id else {
                return Ok(false);
            };
            let row = sqlx::query_as::<_, (bool, bool, String, String, String, String)>(
                "SELECT \
                   ($7 = quoting.uri OR $7 = quoting.url OR \
                    $7 = $10 || '/actor/statuses/' || quoting.id::text OR \
                    $7 = $10 || '/@' || source.username || '/' || quoting.id::text OR \
                    $7 = $10 || '/users/' || source.username || '/statuses/' || quoting.id::text OR \
                    $7 = $10 || '/ap/users/' || source.id::text || '/statuses/' || quoting.id::text), \
                   ($8 = quoted.uri OR $8 = quoted.url), target.uri, target.inbox_url, \
                   target.shared_inbox_url, target.domain \
                 FROM quotes quote \
                 JOIN statuses quoting ON quoting.id = quote.status_id \
                 JOIN accounts source ON source.id = quoting.account_id \
                 JOIN statuses quoted ON quoted.id = quote.quoted_status_id \
                 JOIN accounts target ON target.id = quoted.account_id \
                WHERE quote.id = $1 AND quote.status_id = $2 AND quote.quoted_status_id = $3 \
                  AND quote.activity_uri = $4 AND quote.state = 0 \
                  AND quote.approval_uri IS NULL AND quoting.deleted_at IS NULL \
                  AND quoted.deleted_at IS NULL AND quoting.local IS TRUE \
                  AND quoting.account_id = $5 AND source.domain IS NULL \
                  AND source.suspended_at IS NULL \
                  AND target.domain IS NOT NULL AND target.protocol = 1 \
                  AND target.suspended_at IS NULL \
                  AND quote.quoted_account_id = target.id AND $6 = $9 \
                FOR SHARE OF quote",
            )
            .bind(quote_id)
            .bind(quoting_status_id)
            .bind(identity.quoted_status_id)
            .bind(&identity.request_uri)
            .bind(source_account_id)
            .bind(request_actor_uri)
            .bind(request_instrument_uri)
            .bind(request_target_uri)
            .bind(source_actor_uri)
            .bind(origin)
            .fetch_optional(&mut **transaction)
            .await?;
            Ok(row.is_some_and(
                |(instrument_matches, target_matches, _, direct_inbox, shared_inbox, domain)| {
                    instrument_matches
                        && target_matches
                        && (inbox_url == direct_inbox || inbox_url == shared_inbox)
                        && configured_remote_domain.is_none_or(|value| value == domain)
                },
            ))
        }
        QuoteDeliveryKind::Accept | QuoteDeliveryKind::Reject
            if identity.quote_id.is_some() && identity.quoting_status_id.is_some() =>
        {
            let expected_state = if identity.kind == QuoteDeliveryKind::Accept {
                1
            } else {
                2
            };
            let row = sqlx::query_as::<_, (bool, bool, String, String, String, String)>(
                "SELECT ($7 = instrument.uri OR $7 = instrument.url), \
                        ($8 = target.uri OR $8 = target.url OR \
                         $8 = $10 || '/actor/statuses/' || target.id::text OR \
                         $8 = $10 || '/@' || source.username || '/' || target.id::text OR \
                         $8 = $10 || '/users/' || source.username || '/statuses/' || target.id::text OR \
                         $8 = $10 || '/ap/users/' || source.id::text || '/statuses/' || target.id::text), \
                        requester.uri, requester.inbox_url, requester.shared_inbox_url, requester.domain \
                   FROM quotes quote \
                   JOIN statuses instrument ON instrument.id = quote.status_id \
                   JOIN accounts requester ON requester.id = instrument.account_id \
                   JOIN statuses target ON target.id = quote.quoted_status_id \
                   JOIN accounts source ON source.id = target.account_id \
                  WHERE quote.id = $1 AND quote.status_id = $2 AND quote.quoted_status_id = $3 \
                    AND quote.activity_uri = $4 \
                    AND (($11 = 1 AND quote.state = 1) \
                      OR ($11 = 2 AND quote.state IN (2, 3))) \
                    AND quote.approval_uri IS NULL AND quote.quoted_account_id = $5 \
                    AND instrument.deleted_at IS NULL AND instrument.local IS NOT TRUE \
                    AND target.deleted_at IS NULL AND target.local IS TRUE \
                    AND target.account_id = $5 AND source.domain IS NULL \
                    AND source.suspended_at IS NULL \
                    AND requester.domain IS NOT NULL AND requester.protocol = 1 \
                    AND requester.suspended_at IS NULL AND $6 = $9 \
                  FOR SHARE OF quote",
            )
            .bind(identity.quote_id.expect("checked above"))
            .bind(identity.quoting_status_id.expect("checked above"))
            .bind(identity.quoted_status_id)
            .bind(&identity.request_uri)
            .bind(source_account_id)
            .bind(request_actor_uri)
            .bind(request_instrument_uri)
            .bind(request_target_uri)
            .bind(source_actor_uri)
            .bind(origin)
            .bind(expected_state)
            .fetch_optional(&mut **transaction)
            .await?;
            let Some((
                instrument_matches,
                target_matches,
                requester_uri,
                direct_inbox,
                shared_inbox,
                domain,
            )) = row
            else {
                return Ok(false);
            };
            if !instrument_matches
                || !target_matches
                || request_actor_uri != requester_uri
                || (inbox_url != direct_inbox && inbox_url != shared_inbox)
                || configured_remote_domain.is_some_and(|value| value != domain)
            {
                return Ok(false);
            }
            if identity.kind == QuoteDeliveryKind::Accept {
                let expected_authorization = format!(
                    "{}/quote_authorizations/{quote_id}",
                    source_actor_uri.trim_end_matches('/'),
                    quote_id = identity.quote_id.expect("checked above")
                );
                if quote_body_uri(body.get("result")) != Some(expected_authorization.as_str()) {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        QuoteDeliveryKind::Reject => {
            let row = sqlx::query_as::<_, (bool, String, String, String, String)>(
                "SELECT ($3 = target.uri OR $3 = target.url OR \
                         $3 = $5 || '/actor/statuses/' || target.id::text OR \
                         $3 = $5 || '/@' || source.username || '/' || target.id::text OR \
                         $3 = $5 || '/users/' || source.username || '/statuses/' || target.id::text OR \
                         $3 = $5 || '/ap/users/' || source.id::text || '/statuses/' || target.id::text), \
                        requester.uri, requester.inbox_url, requester.shared_inbox_url, requester.domain \
                   FROM statuses target \
                   JOIN accounts source ON source.id = target.account_id \
                   JOIN accounts requester ON requester.uri = $4 AND requester.domain IS NOT NULL \
                    AND requester.protocol = 1 AND requester.suspended_at IS NULL \
                  WHERE target.id = $1 AND target.account_id = $2 \
                    AND target.deleted_at IS NULL AND target.local IS TRUE \
                    AND source.domain IS NULL AND source.suspended_at IS NULL \
                  ORDER BY requester.id LIMIT 1",
            )
            .bind(identity.quoted_status_id)
            .bind(source_account_id)
            .bind(request_target_uri)
            .bind(request_actor_uri)
            .bind(origin)
            .fetch_optional(&mut **transaction)
            .await?;
            Ok(row.is_some_and(
                |(target_matches, requester_uri, direct_inbox, shared_inbox, domain)| {
                    target_matches
                        && request_actor_uri == requester_uri
                        && (inbox_url == direct_inbox || inbox_url == shared_inbox)
                        && configured_remote_domain.is_none_or(|value| value == domain)
                        && Url::parse(request_instrument_uri).is_ok_and(|instrument| {
                            Url::parse(request_actor_uri)
                                .is_ok_and(|actor| same_url_origin(&instrument, &actor))
                        })
                },
            ))
        }
        QuoteDeliveryKind::Accept => Ok(false),
    }
}

#[allow(clippy::too_many_lines)]
async fn deliver_activity(
    pool: PgPool,
    operational_pool: PgPool,
    config: &ActivityPubDeliveryConfig,
    fetcher: &RemoteFetcher,
    job: &ClaimedJob,
) -> Result<(), HandlerFailure> {
    let arguments = &job.arguments;
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
    let Ok(quote_delivery_identity) = quote_delivery_identity(arguments, body_value) else {
        return Err(HandlerFailure::permanent(
            "quote delivery metadata does not match its activity",
        ));
    };
    let body = serde_json::to_vec(body_value)
        .map_err(|_| HandlerFailure::permanent("delivery activity could not be serialized"))?;
    let repository = Repository::from_pool(pool.clone());
    let delivery_edited_at_micros = arguments.get("edited_at_micros").and_then(Value::as_i64);
    let delivery_poll_updated_at_micros = arguments
        .get("poll_updated_at_micros")
        .and_then(Value::as_i64);
    let delivery_quote_updated_at_micros = arguments
        .get("quote_updated_at_micros")
        .and_then(Value::as_i64);
    let delivery_update_version_micros = arguments
        .get("update_version_micros")
        .and_then(Value::as_i64);
    let delivery_update_kind = arguments.get("update_kind").and_then(Value::as_str);
    let delivery_published_version_micros = body_value
        .get("published")
        .and_then(Value::as_str)
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.timestamp_micros());
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
        if !update_delivery_is_current(
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
        if body_value
            .get("object")
            .and_then(|object| object.get("id"))
            .and_then(Value::as_str)
            != Some(object_uri.as_str())
        {
            return Ok(());
        }
        let current_poll = if let Some(poll_id) = status.poll_id {
            let Some(current) = sqlx::query_scalar::<_, NaiveDateTime>(
                "SELECT updated_at FROM polls WHERE id = $1",
            )
            .bind(poll_id)
            .fetch_optional(&pool)
            .await
            .map_err(|_| HandlerFailure::retry("poll delivery fence lookup failed"))?
            else {
                return Ok(());
            };
            Some((poll_id, current))
        } else {
            None
        };
        let current_edited_at = if delivery_update_kind == Some("interaction_policy") {
            status.updated_at
        } else {
            status.edited_at.unwrap_or(status.updated_at)
        };
        let current_poll_updated_at = current_poll.map(|(_, updated_at)| updated_at);
        let quote_delivery_is_current = if delivery_update_kind == Some("quote") {
            let current_quote_updated_at = sqlx::query_scalar::<_, NaiveDateTime>(
                "SELECT updated_at FROM quotes WHERE status_id = $1 ORDER BY id LIMIT 1",
            )
            .bind(status.id)
            .fetch_optional(&pool)
            .await
            .map_err(|_| HandlerFailure::retry("quote delivery fence lookup failed"))?;
            quote_revision_is_current(current_quote_updated_at, delivery_quote_updated_at_micros)
        } else {
            true
        };
        let complete_delivery_kind = complete_status_update_delivery_kind(
            delivery_update_kind,
            current_poll.is_some(),
            delivery_edited_at_micros,
            delivery_poll_updated_at_micros,
            delivery_update_version_micros,
            delivery_published_version_micros,
        );
        let inferred_repair_kind = complete_delivery_kind.is_none().then(|| {
            inferred_current_repair_delivery_kind(
                body_value["id"].as_str(),
                &object_uri,
                current_edited_at,
                current_poll_updated_at,
                delivery_published_version_micros,
            )
        });
        let delivery_is_current = quote_delivery_is_current
            && (complete_delivery_kind.is_some_and(|delivery_kind| {
                status_update_delivery_is_current(
                    delivery_kind,
                    body_value["id"].as_str(),
                    &object_uri,
                    current_edited_at,
                    current_poll_updated_at,
                    delivery_edited_at_micros,
                    delivery_poll_updated_at_micros,
                    delivery_update_version_micros,
                )
            }) || inferred_repair_kind.flatten().is_some());
        if !delivery_is_current {
            if delivery_update_kind == Some("quote") {
                return Ok(());
            }
            queue_status_snapshot_repair(
                &pool,
                status.id,
                current_poll.map(|(poll_id, _)| poll_id),
                current_edited_at,
                current_poll_updated_at,
            )
            .await?;
            return Ok(());
        }
    }
    let private_key = source_account
        .private_key
        .as_ref()
        .filter(|key| key.is_present())
        .ok_or_else(|| HandlerFailure::permanent("delivery source has no private key"))?;
    let source_actor_uri = activitypub::actor_url(&config.origin, &source_account);
    let key_id = format!("{source_actor_uri}#main-key");
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
    let delivery = if let Some(identity) = quote_delivery_identity.as_ref() {
        let mut transaction = pool
            .begin()
            .await
            .map_err(|_| HandlerFailure::retry("quote delivery fence transaction failed"))?;
        let current = quote_delivery_is_current_and_locked(
            &mut transaction,
            identity,
            body_value,
            source_account_id,
            &source_actor_uri,
            inbox_url.as_str(),
            configured_remote_domain.as_deref(),
            config.origin.as_str(),
        )
        .await
        .map_err(|_| HandlerFailure::retry("quote delivery fence lookup failed"))?;
        if !current {
            transaction
                .rollback()
                .await
                .map_err(|_| HandlerFailure::retry("quote delivery fence rollback failed"))?;
            return Ok(());
        }
        if !WriteRepository::remote_domain_allowed_in_transaction(
            &mut transaction,
            policy_domain,
            config.limited_federation,
        )
        .await
        .map_err(|_| HandlerFailure::retry("quote delivery policy fence failed"))?
        {
            transaction
                .rollback()
                .await
                .map_err(|_| HandlerFailure::retry("quote delivery fence rollback failed"))?;
            return Ok(());
        }
        let lease_fenced = sqlx::query(
            "UPDATE rustodon.durable_jobs \
                SET lease_expires_at = GREATEST(lease_expires_at, \
                    clock_timestamp() + interval '60 seconds'), \
                    updated_at = clock_timestamp() \
              WHERE id = $1 AND lease_owner = $2 AND lease_generation = $3 \
                AND dead_at IS NULL AND lease_expires_at > clock_timestamp()",
        )
        .bind(job.id)
        .bind(&job.lease_owner)
        .bind(job.generation)
        .execute(&mut *transaction)
        .await
        .map_err(|_| HandlerFailure::retry("quote delivery lease fence failed"))?
        .rows_affected()
            == 1;
        if !lease_fenced {
            transaction
                .rollback()
                .await
                .map_err(|_| HandlerFailure::retry("quote delivery fence rollback failed"))?;
            return Ok(());
        }
        #[cfg(feature = "test-support")]
        let delivery = if let Some(endpoint) = config.remote_delivery_endpoint {
            fetcher
                .post_signed_json_for_test_endpoint(inbox_url.clone(), &body, &signer, endpoint)
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
        if delivery.is_ok() {
            transaction
                .commit()
                .await
                .map_err(|_| HandlerFailure::retry("quote delivery fence commit failed"))?;
        } else {
            transaction
                .rollback()
                .await
                .map_err(|_| HandlerFailure::retry("quote delivery fence rollback failed"))?;
        }
        delivery
    } else if is_delete && status_id.is_none() {
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
        | RemoteFetchError::PolicyDenied
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
        | RemoteFetchError::PolicyDenied
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
        | RemoteFetchError::PolicyDenied
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
        | RemoteFetchError::PolicyDenied
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
        | WriteError::Forbidden
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
    if !["Note", "Question"]
        .into_iter()
        .any(|kind| equals_or_includes(document.get("type"), kind))
    {
        return Err(HandlerFailure::permanent(
            "remote Create object is not a Note or Question",
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

fn remote_quote_request_fetch_failure(error: &RemoteFetchError) -> HandlerFailure {
    match error {
        RemoteFetchError::UnexpectedStatus(status)
            if *status == StatusCode::NOT_FOUND
                || *status == StatusCode::REQUEST_TIMEOUT
                || *status == StatusCode::TOO_MANY_REQUESTS
                || status.is_server_error() =>
        {
            HandlerFailure::retry(format!(
                "QuoteRequest instrument is temporarily unavailable: {error}"
            ))
        }
        RemoteFetchError::UnexpectedStatus(_) => HandlerFailure::permanent(format!(
            "QuoteRequest instrument fetch was rejected: {error}"
        )),
        RemoteFetchError::NoAddresses
        | RemoteFetchError::Dns
        | RemoteFetchError::Client
        | RemoteFetchError::Request
        | RemoteFetchError::BodyRead
        | RemoteFetchError::DomainBudgetExceeded => {
            HandlerFailure::retry(format!("QuoteRequest instrument fetch failed: {error}"))
        }
        RemoteFetchError::InvalidUrl
        | RemoteFetchError::BlockedAddress(_)
        | RemoteFetchError::Redirect
        | RemoteFetchError::TooManyRedirects
        | RemoteFetchError::MissingContentType
        | RemoteFetchError::UnsupportedContentType
        | RemoteFetchError::UnsupportedEncoding
        | RemoteFetchError::BodyTooLarge
        | RemoteFetchError::InvalidRepresentation
        | RemoteFetchError::IdentityMismatch
        | RemoteFetchError::OriginMismatch
        | RemoteFetchError::PolicyDenied
        | RemoteFetchError::Signing => {
            HandlerFailure::permanent(format!("QuoteRequest instrument fetch is invalid: {error}"))
        }
    }
}

async fn fetch_quote_request_instrument(
    repository: &Repository,
    config: &ActivityPubDeliveryConfig,
    fetcher: &RemoteFetcher,
    target_account_id: i64,
    instrument_uri: &str,
) -> Result<Value, HandlerFailure> {
    let parsed_instrument = Url::parse(instrument_uri)
        .map_err(|_| HandlerFailure::permanent("QuoteRequest instrument URL is invalid"))?;
    let signer_account = repository
        .account(target_account_id)
        .await
        .map_err(|_| HandlerFailure::retry("QuoteRequest fetch signer lookup failed"))?
        .filter(|account| account.domain.is_none())
        .ok_or_else(|| HandlerFailure::permanent("QuoteRequest fetch signer is unavailable"))?;
    let private_key = signer_account
        .private_key
        .as_ref()
        .filter(|key| key.is_present())
        .ok_or_else(|| HandlerFailure::permanent("QuoteRequest fetch signer has no private key"))?;
    let signer_key_id = format!(
        "{}#main-key",
        activitypub::actor_url(&config.origin, &signer_account)
    );
    let signer = HttpSignatureSigner {
        key_id: &signer_key_id,
        private_key_pem: private_key.as_str(),
    };
    #[cfg(feature = "test-support")]
    let fetcher = fetcher
        .clone()
        .with_test_endpoint(config.remote_fetch_endpoint);
    #[cfg(not(feature = "test-support"))]
    let fetcher = fetcher.clone();
    let response = fetcher
        .get_signed(parsed_instrument, THREAD_ACTIVITYPUB_CONTENT_TYPES, &signer)
        .await
        .map_err(|error| remote_quote_request_fetch_failure(&error))?;
    serde_json::from_slice(&response.body)
        .map_err(|_| HandlerFailure::permanent("QuoteRequest instrument JSON is invalid"))
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
    let repository = Repository::from_pool(pool.clone());
    let writer = WriteRepository::from_pool(pool.clone());
    let resolved = writer
        .remote_note_reference_is_resolved(source_account_id, actor_uri, object_uri)
        .await
        .map_err(|error| {
            remote_note_write_failure(&error, "remote Note resolution state lookup failed")
        })?;
    if resolved && let Some(delivery_target_account_id) = delivery_target_account_id {
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
    let target = Url::parse(object_uri)
        .map_err(|_| HandlerFailure::permanent("remote Create object URI is invalid"))?;
    if same_url_origin(&target, &config.origin) {
        return Err(HandlerFailure::permanent(
            "remote Create object URI is local",
        ));
    }
    let object_domain = inbox_actor_domain(object_uri)
        .ok_or_else(|| HandlerFailure::permanent("remote Create object has no valid domain"))?;
    if !repository
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
    let quote_authorization = fetch_and_import_quote_authorization(
        &pool,
        &repository,
        &writer,
        config,
        fetcher,
        source_account_id,
        delivery_target_account_id,
        &object,
    )
    .await?;
    if resolved {
        writer
            .apply_remote_note_update(
                source_account_id,
                actor_uri,
                &object,
                delivery_target_account_id,
                config.origin.as_str(),
            )
            .await
            .map_err(|error| {
                remote_note_write_failure(&error, "resolved remote Note replay write failed")
            })?;
    } else {
        writer
            .apply_remote_note_create(
                source_account_id,
                actor_uri,
                &object,
                delivery_target_account_id,
                config.origin.as_str(),
            )
            .await
            .map_err(|error| {
                remote_note_write_failure(&error, "resolved remote Note write failed")
            })?;
    }
    if let Some((references, Some(document))) = quote_authorization {
        let approval_uri = references
            .approval_uri
            .as_deref()
            .expect("fetched authorization has a canonical URI");
        writer
            .apply_remote_quote_authorization(
                source_account_id,
                object_uri,
                approval_uri,
                &document,
                config.origin.as_str(),
            )
            .await
            .map_err(|error| {
                remote_note_write_failure(&error, "resolved remote QuoteAuthorization write failed")
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
    let (object, actor_uri, target_uri) = if ["Note", "Question"]
        .into_iter()
        .any(|kind| equals_or_includes(document.get("type"), kind))
    {
        let actor_uri = remote_uri_value(document.get("attributedTo")).ok_or_else(|| {
            HandlerFailure::permanent("remote Announce target Note has no author")
        })?;
        if remote_uri_value(document.get("id")) != Some(object_uri) {
            return Err(HandlerFailure::permanent(
                "remote Announce target ID does not match the requested URI",
            ));
        }
        (document, actor_uri, object_uri)
    } else if equals_or_includes(document.get("type"), "Create") {
        if remote_uri_value(document.get("id")) != Some(object_uri) {
            return Err(HandlerFailure::permanent(
                "remote Announce target Create ID does not match the requested URI",
            ));
        }
        let object = document.get("object").ok_or_else(|| {
            HandlerFailure::permanent("remote Announce target Create has no object")
        })?;
        if !["Note", "Question"]
            .into_iter()
            .any(|kind| equals_or_includes(object.get("type"), kind))
        {
            return Err(HandlerFailure::permanent(
                "remote Announce target Create object is not a Note or Question",
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
        let target_uri = remote_uri_value(object.get("id"))
            .ok_or_else(|| HandlerFailure::permanent("remote Announce target Note has no ID"))?;
        (object, actor_uri, target_uri)
    } else {
        return Err(HandlerFailure::permanent(
            "remote Announce target is not a Note, Question, or Create",
        ));
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
    if !equals_or_includes(document.get("type"), "Announce") {
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
        ["Note", "Question"]
            .into_iter()
            .any(|kind| equals_or_includes(object.get("type"), kind))
            .then(|| {
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
    let (object, actor_uri) = if ["Note", "Question"]
        .into_iter()
        .any(|kind| equals_or_includes(document.get("type"), kind))
    {
        let actor_uri = remote_uri_value(document.get("attributedTo"))
            .ok_or_else(|| HandlerFailure::permanent("thread resolution Note has no author"))?;
        (&document, actor_uri)
    } else if equals_or_includes(document.get("type"), "Create") {
        let object = document
            .get("object")
            .ok_or_else(|| HandlerFailure::permanent("thread resolution Create has no object"))?;
        if !["Note", "Question"]
            .into_iter()
            .any(|kind| equals_or_includes(object.get("type"), kind))
        {
            return Err(HandlerFailure::permanent(
                "thread resolution Create object is not a Note or Question",
            ));
        }
        let actor_uri = remote_uri_value(document.get("actor"))
            .ok_or_else(|| HandlerFailure::permanent("thread resolution Create has no actor"))?;
        if remote_uri_value(object.get("attributedTo")) != Some(actor_uri) {
            return Err(HandlerFailure::permanent(
                "thread resolution Note author does not match Create actor",
            ));
        }
        (object, actor_uri)
    } else {
        return Err(HandlerFailure::permanent(
            "thread resolution parent is not a Note, Question, or Create",
        ));
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
            Option<String>,
            Option<String>,
        ),
    >(
        "SELECT media.account_id, media.remote_url, media.file_file_name,
                media.file_content_type, media.blurhash, account.domain
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
    #[cfg(feature = "test-support")]
    let fetcher = &fetcher
        .clone()
        .with_test_endpoint(config.remote_media_endpoint);
    let media_response = match RemoteMediaFetcher::new(fetcher, existing_content_type.as_deref()) {
        Ok(fetcher) => {
            fetcher
                .get_with_policy(remote_url.clone(), |url| {
                    let repository = Repository::from_pool(pool.clone());
                    async move {
                        let domain = canonical_remote_domain_from_url(&url)?;
                        if repository
                            .remote_media_allowed(&domain, config.limited_federation)
                            .await
                            .map_err(|_| RemoteFetchError::Request)?
                        {
                            Ok(())
                        } else {
                            Err(RemoteFetchError::PolicyDenied)
                        }
                    }
                })
                .await
        }
        Err(error) => Err(error),
    };
    let (response, visited) = match media_response {
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
        match prepare_rich_media_attachment(account_id, file_name, &content_type, &response.body)
            .await
        {
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
    let visited_domains = visited
        .iter()
        .map(canonical_remote_domain_from_url)
        .collect::<Result<BTreeSet<_>, _>>()
        .map_err(|_| HandlerFailure::permanent("remote media hop has no valid domain"))?;
    let install_marker = RemoteMediaInstallMarker {
        file_name: prepared.file_name.clone(),
        content_type: prepared.content_type.clone(),
        file_size: prepared.file_size,
    };
    let transaction_install_marker = install_marker.clone();
    let metadata = PaperclipMetadata {
        attachment: PaperclipAttachment::MediaFile,
        id: media_id,
        remote: true,
        storage_schema_version: Some(1),
        file_name: prepared.file_name.clone(),
        content_type: Some(prepared.content_type.clone()),
        variant: None,
    };
    let writer = WriteRepository::from_pool(pool.clone());
    let persisted = writer
        .with_remote_domain_locks(&account_domain, || async {
            let mut transaction = pool.begin().await?;
            let mut pending_stream_events = Vec::new();
            let current = sqlx::query_as::<
                _,
                (
                    String,
                    Option<String>,
                    Option<String>,
                    bool,
                    Option<i64>,
                    Option<Value>,
                ),
            >(
                "SELECT media.remote_url, media.file_file_name, account.domain,
                        status.deleted_at IS NULL, media.account_id, media.file_meta
                   FROM media_attachments media
                   JOIN accounts account ON account.id = media.account_id
                   JOIN statuses status ON status.id = media.status_id
                  WHERE media.id = $1
                  FOR UPDATE OF media, account, status",
            )
            .bind(media_id)
            .fetch_optional(&mut *transaction)
            .await?;
            let Some((
                current_remote_url,
                current_file_name,
                current_domain,
                active,
                current_account_id,
                current_meta,
            )) = current
            else {
                transaction.rollback().await?;
                return Ok(false);
            };
            if !active
                || current_account_id != Some(account_id)
                || current_remote_url != remote_url.as_str()
                || current_domain.as_deref() != Some(account_domain.as_str())
                || current_file_name.is_some()
            {
                transaction.rollback().await?;
                return Ok(false);
            }
            let mut allowed = writer
                .remote_media_allowed_in_transaction(
                    &mut transaction,
                    &remote_domain,
                    config.limited_federation,
                )
                .await?;
            for domain in &visited_domains {
                allowed &= writer
                    .remote_media_allowed_in_transaction(
                        &mut transaction,
                        domain,
                        config.limited_federation,
                    )
                    .await?;
            }
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
            // Focus can be edited or cleared while fetching. Merge the current
            // locked row, never the prefetch snapshot, without replacing measured geometry.
            let mut file_meta = prepared.file_meta.clone();
            if let (Value::Object(target), Some(Value::Object(source))) =
                (&mut file_meta, current_meta)
            {
                for (key, value) in source {
                    if !matches!(key.as_str(), "original" | "small") {
                        target.insert(key, value);
                    }
                }
            }
            let written_paths = write_prepared_media(&media_root, &metadata, &prepared)?;
            let mut written_files = WrittenMediaFiles::new(&media_root, written_paths);
            let status_id = match sqlx::query_scalar::<_, i64>(
                "UPDATE media_attachments SET processing = 2, file_content_type = $3,
                    file_file_name = $4, file_file_size = $5, file_meta = $6::json,
                    file_storage_schema_version = 1, file_updated_at = clock_timestamp(),
                    blurhash = COALESCE($7, blurhash), type = $8, updated_at = clock_timestamp()
                  WHERE id = $1 AND status_id IS NOT NULL AND remote_url = $2
                    AND EXISTS (
                        SELECT 1 FROM statuses
                         WHERE statuses.id = media_attachments.status_id
                           AND statuses.deleted_at IS NULL
                    )
                  RETURNING status_id",
            )
            .bind(media_id)
            .bind(remote_url.as_str())
            .bind(&prepared.content_type)
            .bind(&prepared.file_name)
            .bind(prepared.file_size)
            .bind(file_meta)
            .bind(blurhash.or(prepared.blurhash))
            .bind(prepared.media_kind.database_type())
            .fetch_optional(&mut *transaction)
            .await
            {
                Ok(status_id) => status_id,
                Err(error) => {
                    written_files.cleanup();
                    let _ = transaction.rollback().await;
                    return Err(error.into());
                }
            };
            let Some(status_id) = status_id else {
                written_files.cleanup();
                transaction.commit().await?;
                return Ok(false);
            };
            WriteRepository::collect_remote_media_installed_stream_events_in(
                &mut transaction,
                &mut pending_stream_events,
                status_id,
                media_id,
            )
            .await?;
            // Until COMMIT is issued, cancellation must still remove the files.
            if let Err(error) =
                flush_stream_events_in(&mut transaction, &mut pending_stream_events).await
            {
                written_files.cleanup();
                let _ = transaction.rollback().await;
                return Err(error.into());
            }
            // A cancelled or failed COMMIT is ambiguous: PostgreSQL may still commit after this
            // future is dropped. Disarm only immediately before COMMIT; retain paths until the
            // synchronized probe below proves that the installation rolled back.
            #[cfg(feature = "test-support")]
            let (written_paths, commit_result) = if media_root.take_commit_before_fault() {
                transaction.rollback().await?;
                (
                    written_files.preserve(),
                    Err(sqlx::Error::Protocol(
                        "injected ambiguous metadata commit failure".to_owned(),
                    )),
                )
            } else {
                let written_paths = written_files.preserve();
                (written_paths, transaction.commit().await)
            };
            #[cfg(not(feature = "test-support"))]
            let (written_paths, commit_result) = {
                let written_paths = written_files.preserve();
                (written_paths, transaction.commit().await)
            };
            #[cfg(feature = "test-support")]
            let commit_result = if commit_result.is_ok() && media_root.take_commit_after_fault() {
                Err(sqlx::Error::Protocol(
                    "injected ambiguous metadata commit result".to_owned(),
                ))
            } else {
                commit_result
            };
            match commit_result {
                Ok(()) => Ok(true),
                Err(error) => {
                    match remote_media_install_committed(
                        &pool,
                        media_id,
                        remote_url.as_str(),
                        &transaction_install_marker,
                    )
                    .await
                    {
                        Ok(true) => Ok(true),
                        Ok(false) => {
                            WrittenMediaFiles::new(&media_root, written_paths).cleanup();
                            Err(error.into())
                        }
                        Err(_) => {
                            // A failed reconciliation leaves the commit genuinely ambiguous.
                            // Preserve files because deleting them could corrupt an installation
                            // that committed.
                            Err(error.into())
                        }
                    }
                }
            }
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
            let committed = remote_media_install_committed(
                &pool,
                media_id,
                remote_url.as_str(),
                &install_marker,
            )
            .await
            .is_ok_and(|committed| committed);
            if committed {
                Ok(())
            } else if attempt >= max_attempts {
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

#[derive(Clone)]
struct RemoteMediaInstallMarker {
    file_name: String,
    content_type: String,
    file_size: i32,
}

async fn remote_media_install_committed(
    pool: &PgPool,
    media_id: i64,
    remote_url: &str,
    marker: &RemoteMediaInstallMarker,
) -> Result<bool, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    let installed = sqlx::query_as::<
        _,
        (
            String,
            Option<i32>,
            Option<String>,
            Option<String>,
            Option<i32>,
            Option<i32>,
        ),
    >(
        "SELECT remote_url, processing, file_file_name, file_content_type,
                file_file_size, file_storage_schema_version
           FROM media_attachments WHERE id = $1 FOR UPDATE",
    )
    .bind(media_id)
    .fetch_optional(&mut *transaction)
    .await?;
    let committed = installed.is_some_and(
        |(current_url, processing, file_name, content_type, file_size, storage_schema_version)| {
            current_url == remote_url
                && processing == Some(2)
                && file_name.as_deref() == Some(marker.file_name.as_str())
                && content_type.as_deref() == Some(marker.content_type.as_str())
                && file_size == Some(marker.file_size)
                && storage_schema_version == Some(1)
        },
    );
    transaction.rollback().await?;
    Ok(committed)
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

    fn preserve(&mut self) -> Vec<String> {
        std::mem::take(&mut self.paths)
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

fn remote_poll_vote_allows_note_fallback(outcome: RemotePollVoteOutcome) -> bool {
    outcome == RemotePollVoteOutcome::NotPollVote
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn fetch_and_import_quote_authorization(
    pool: &PgPool,
    repository: &Repository,
    writer: &WriteRepository,
    config: &ActivityPubDeliveryConfig,
    fetcher: &RemoteFetcher,
    source_account_id: i64,
    delivery_target_account_id: Option<i64>,
    object: &Value,
) -> Result<Option<(RemoteQuoteFetchReferences, Option<Value>)>, HandlerFailure> {
    let Some(references) = remote_note_quote_fetch_references(object) else {
        return Ok(None);
    };
    let existing_target_local = writer
        .remote_quote_target_is_local(&references.target_uri, config.origin.as_str())
        .await
        .map_err(|error| remote_note_write_failure(&error, "quoted status lookup failed"))?;
    if existing_target_local == Some(true) {
        return Ok(Some((references, None)));
    }
    let target_url = Url::parse(&references.target_uri)
        .map_err(|_| HandlerFailure::permanent("quoted status URL is invalid"))?;
    let target_domain = canonical_remote_domain_from_url(&target_url)
        .map_err(|_| HandlerFailure::permanent("quoted status domain is invalid"))?;
    if !repository
        .remote_domain_allowed(&target_domain, config.limited_federation)
        .await
        .map_err(|_| HandlerFailure::retry("quoted status domain policy lookup failed"))?
    {
        return Err(HandlerFailure::permanent(
            "quoted status domain is not allowed",
        ));
    }
    let quote_to = remote_announce_audience(object, "to")?;
    let quote_cc = remote_announce_audience(object, "cc")?;
    let signer_account = resolve_note_fetch_signer(
        pool,
        source_account_id,
        delivery_target_account_id,
        &quote_to,
        &quote_cc,
        &config.origin,
    )
    .await?;
    let private_key = signer_account
        .private_key
        .as_ref()
        .filter(|key| key.is_present())
        .ok_or_else(|| HandlerFailure::permanent("quote fetch signer has no private key"))?;
    let signer_key_id = format!(
        "{}#main-key",
        activitypub::actor_url(&config.origin, &signer_account)
    );
    let signer = HttpSignatureSigner {
        key_id: &signer_key_id,
        private_key_pem: private_key.as_str(),
    };
    let authorization_document = if let Some(approval_uri) = references.approval_uri.as_deref() {
        let approval_url = Url::parse(approval_uri)
            .map_err(|_| HandlerFailure::permanent("quote authorization URL is invalid"))?;
        let approval_domain = canonical_remote_domain_from_url(&approval_url)
            .map_err(|_| HandlerFailure::permanent("quote authorization domain is invalid"))?;
        if !repository
            .remote_domain_allowed(&approval_domain, config.limited_federation)
            .await
            .map_err(|_| HandlerFailure::retry("quote authorization domain policy lookup failed"))?
        {
            return Err(HandlerFailure::permanent(
                "quote authorization domain is not allowed",
            ));
        }
        if !same_url_origin(&approval_url, &target_url) {
            return Err(HandlerFailure::permanent(
                "quoted status does not match the authorization origin",
            ));
        }
        let response = {
            #[cfg(feature = "test-support")]
            if let Some(endpoint) = config.remote_fetch_endpoint {
                fetcher
                    .get_for_test_endpoint(
                        approval_url.clone(),
                        THREAD_ACTIVITYPUB_CONTENT_TYPES,
                        endpoint,
                    )
                    .await
            } else {
                fetcher
                    .get_signed(
                        approval_url.clone(),
                        THREAD_ACTIVITYPUB_CONTENT_TYPES,
                        &signer,
                    )
                    .await
            }
            #[cfg(not(feature = "test-support"))]
            fetcher
                .get_signed(
                    approval_url.clone(),
                    THREAD_ACTIVITYPUB_CONTENT_TYPES,
                    &signer,
                )
                .await
        }
        .map_err(|error| remote_note_fetch_failure(&error))?;
        if !same_url_origin(&response.url, &approval_url) {
            return Err(HandlerFailure::permanent(
                "quote authorization redirected to another origin",
            ));
        }
        Some(
            serde_json::from_slice::<Value>(&response.body)
                .map_err(|_| HandlerFailure::permanent("quote authorization JSON is invalid"))?,
        )
    } else {
        None
    };
    let target_exists = existing_target_local.is_some();
    if !target_exists {
        let embedded_authorization_target = authorization_document
            .as_ref()
            .and_then(|document| document.get("interactionTarget"))
            .filter(|target| target.is_object())
            .cloned();
        let target_document = if let Some(target) = embedded_authorization_target {
            target
        } else {
            let response = {
                #[cfg(feature = "test-support")]
                if let Some(endpoint) = config.remote_fetch_endpoint {
                    fetcher
                        .get_for_test_endpoint(
                            target_url.clone(),
                            THREAD_ACTIVITYPUB_CONTENT_TYPES,
                            endpoint,
                        )
                        .await
                } else {
                    fetcher
                        .get_signed(
                            target_url.clone(),
                            THREAD_ACTIVITYPUB_CONTENT_TYPES,
                            &signer,
                        )
                        .await
                }
                #[cfg(not(feature = "test-support"))]
                fetcher
                    .get_signed(
                        target_url.clone(),
                        THREAD_ACTIVITYPUB_CONTENT_TYPES,
                        &signer,
                    )
                    .await
            }
            .map_err(|error| remote_note_fetch_failure(&error))?;
            if !same_url_origin(&response.url, &target_url) {
                return Err(HandlerFailure::permanent(
                    "quoted status redirected to another origin",
                ));
            }
            serde_json::from_slice::<Value>(&response.body)
                .map_err(|_| HandlerFailure::permanent("quoted status JSON is invalid"))?
        };
        let (target, actor_uri, canonical_target_uri) =
            remote_note_document(&target_document, &references.target_uri)?;
        if canonical_target_uri != references.target_uri {
            return Err(HandlerFailure::permanent(
                "quoted status identity is invalid",
            ));
        }
        validate_fetched_activity_actor(&references.target_uri, &actor_uri)?;
        let account_id =
            resolve_remote_note_author(pool, config, writer, fetcher, &actor_uri, &signer).await?;
        writer
            .apply_remote_note_create(
                account_id,
                &actor_uri,
                &target,
                delivery_target_account_id,
                config.origin.as_str(),
            )
            .await
            .map_err(|error| remote_note_write_failure(&error, "quoted status import failed"))?;
    }
    Ok(Some((references, authorization_document)))
}

#[allow(clippy::too_many_lines)]
async fn process_activitypub_inbox(
    pool: PgPool,
    config: &ActivityPubDeliveryConfig,
    fetcher: &RemoteFetcher,
    claimed_job: &ClaimedJob,
    report_mail_enabled: bool,
) -> Result<(), HandlerFailure> {
    let job = parse_job_arguments(&claimed_job.arguments)
        .map_err(|error| HandlerFailure::permanent(error.to_string()))?;
    let activity =
        parse_activity(&job.body).map_err(|error| HandlerFailure::permanent(error.to_string()))?;
    if matches!(activity, InboxActivity::Unsupported) {
        return Ok(());
    }
    let (actor_uri, nested_actor_uri) = match &activity {
        InboxActivity::CreateVote { actor_uri, .. }
        | InboxActivity::CreateNote { actor_uri, .. }
        | InboxActivity::CreateNoteReference { actor_uri, .. }
        | InboxActivity::UpdateNote { actor_uri, .. }
        | InboxActivity::DeleteNote { actor_uri, .. }
        | InboxActivity::DeleteQuoteAuthorization { actor_uri, .. }
        | InboxActivity::QuoteRequest { actor_uri, .. }
        | InboxActivity::QuoteDecision { actor_uri, .. }
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
        InboxActivity::CreateVote {
            actor_uri,
            vote_uri,
            question_uri,
            option,
            object,
            activity,
            ..
        } => {
            let outcome = writer
                .apply_remote_poll_vote(
                    source_account_id,
                    &actor_uri,
                    &vote_uri,
                    &question_uri,
                    &option,
                    config.origin.as_str(),
                )
                .await
                .map_err(|error| {
                    remote_note_write_failure(&error, "remote poll vote write failed")
                })?;
            if remote_poll_vote_allows_note_fallback(outcome) {
                let Value::Object(note_object) = &object else {
                    return Err(HandlerFailure::permanent(
                        "remote vote candidate object is not a Note object",
                    ));
                };
                validate_note_object(&actor_uri, note_object)
                    .map_err(|error| HandlerFailure::permanent(error.to_string()))?;
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
                if activity
                    .get("signature")
                    .is_some_and(|signature| !signature.is_null())
                {
                    writer
                        .record_remote_note_forwarding(&actor_uri, &object, &activity)
                        .await
                        .map_err(|error| {
                            remote_note_write_failure(
                                &error,
                                "remote Note forwarding outbox write failed",
                            )
                        })?;
                }
            }
        }
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
        InboxActivity::QuoteRequest {
            request_uri,
            actor_uri,
            object_uri,
            instrument,
        } => {
            let instrument_uri = remote_uri_value(Some(&instrument))
                .ok_or_else(|| HandlerFailure::permanent("QuoteRequest instrument has no ID"))?
                .to_owned();
            let import_target = writer
                .remote_quote_request_may_import(
                    source_account_id,
                    &request_uri,
                    &actor_uri,
                    &object_uri,
                    &instrument_uri,
                    config.origin.as_str(),
                    job.delivery_target_account_id,
                )
                .await
                .map_err(|error| {
                    remote_note_write_failure(&error, "QuoteRequest import authorization failed")
                })?;
            if let Some((target_status_id, target_account_id)) = import_target {
                let materialized_instrument = if instrument.is_object() {
                    instrument
                } else {
                    match fetch_quote_request_instrument(
                        &repository,
                        config,
                        fetcher,
                        target_account_id,
                        &instrument_uri,
                    )
                    .await
                    {
                        Ok(instrument) => instrument,
                        Err(failure)
                            if failure.disposition == FailureDisposition::Retry
                                && claimed_job.attempt >= claimed_job.max_attempts =>
                        {
                            writer
                                .apply_remote_quote_request(
                                    source_account_id,
                                    &request_uri,
                                    &actor_uri,
                                    &object_uri,
                                    &instrument_uri,
                                    config.origin.as_str(),
                                    job.delivery_target_account_id,
                                )
                                .await
                                .map_err(|error| {
                                    remote_note_write_failure(
                                        &error,
                                        "terminal QuoteRequest rejection failed",
                                    )
                                })?;
                            return Ok(());
                        }
                        Err(failure) => return Err(failure),
                    }
                };
                validate_quote_request_instrument(
                    &writer,
                    &materialized_instrument,
                    &actor_uri,
                    &instrument_uri,
                    target_status_id,
                    config.origin.as_str(),
                )
                .await?;
                writer
                    .apply_remote_quote_request_instrument(
                        source_account_id,
                        &actor_uri,
                        &materialized_instrument,
                        job.delivery_target_account_id,
                        config.origin.as_str(),
                        &request_uri,
                        &object_uri,
                        &instrument_uri,
                        target_status_id,
                        target_account_id,
                    )
                    .await
                    .map_err(|error| {
                        remote_note_write_failure(&error, "QuoteRequest instrument write failed")
                    })?;
            }
            writer
                .apply_remote_quote_request(
                    source_account_id,
                    &request_uri,
                    &actor_uri,
                    &object_uri,
                    &instrument_uri,
                    config.origin.as_str(),
                    job.delivery_target_account_id,
                )
                .await
                .map_err(|error| {
                    remote_note_write_failure(&error, "remote QuoteRequest write failed")
                })?;
        }
        InboxActivity::QuoteDecision {
            accepted,
            actor_uri,
            request_uri,
            request_actor_uri,
            object_uri,
            instrument_uri,
            result_uri,
        } => {
            let follow_fallback = quote_decision_allows_follow_fallback(
                accepted,
                request_actor_uri.as_deref(),
                object_uri.as_deref(),
                instrument_uri.as_deref(),
            );
            let quote_matched = writer
                .apply_remote_quote_decision(
                    source_account_id,
                    &actor_uri,
                    &request_uri,
                    request_actor_uri.as_deref(),
                    object_uri.as_deref(),
                    instrument_uri.as_deref(),
                    result_uri.as_deref(),
                    accepted,
                    config.origin.as_str(),
                    job.delivery_target_account_id,
                )
                .await
                .map_err(|error| {
                    remote_note_write_failure(&error, "remote quote decision write failed")
                })?;
            if follow_fallback && !quote_matched {
                writer
                    .apply_remote_follow_decision(
                        source_account_id,
                        &request_uri,
                        None,
                        None,
                        true,
                        config.origin.as_str(),
                        job.delivery_target_account_id,
                    )
                    .await
                    .map_err(|_| HandlerFailure::retry("remote Accept write failed"))?;
            }
        }
        InboxActivity::DeleteQuoteAuthorization {
            actor_uri,
            authorization_uri,
            activity,
        } => {
            let forwarding_activity = activity
                .get("signature")
                .is_some_and(|signature| !signature.is_null())
                .then_some(&activity);
            writer
                .apply_remote_quote_authorization_delete(
                    source_account_id,
                    &actor_uri,
                    &authorization_uri,
                    forwarding_activity,
                )
                .await
                .map_err(|error| {
                    remote_note_write_failure(
                        &error,
                        "remote QuoteAuthorization Delete write failed",
                    )
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
            let quoting_uri = object
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| HandlerFailure::permanent("remote Note has no object URI"))?;
            let quote_authorization = fetch_and_import_quote_authorization(
                &pool,
                &repository,
                &writer,
                config,
                fetcher,
                source_account_id,
                job.delivery_target_account_id,
                &object,
            )
            .await?;
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
            if let Some((references, Some(document))) = quote_authorization {
                let approval_uri = references
                    .approval_uri
                    .as_deref()
                    .expect("fetched authorization has a canonical URI");
                writer
                    .apply_remote_quote_authorization(
                        source_account_id,
                        quoting_uri,
                        approval_uri,
                        &document,
                        config.origin.as_str(),
                    )
                    .await
                    .map_err(|error| {
                        remote_note_write_failure(&error, "remote QuoteAuthorization write failed")
                    })?;
            }
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
            let quoting_uri = object
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| HandlerFailure::permanent("remote Note has no object URI"))?;
            let quote_authorization = fetch_and_import_quote_authorization(
                &pool,
                &repository,
                &writer,
                config,
                fetcher,
                source_account_id,
                job.delivery_target_account_id,
                &object,
            )
            .await?;
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
            if let Some((references, Some(document))) = quote_authorization {
                let approval_uri = references
                    .approval_uri
                    .as_deref()
                    .expect("fetched authorization has a canonical URI");
                writer
                    .apply_remote_quote_authorization(
                        source_account_id,
                        quoting_uri,
                        approval_uri,
                        &document,
                        config.origin.as_str(),
                    )
                    .await
                    .map_err(|error| {
                        remote_note_write_failure(&error, "remote QuoteAuthorization write failed")
                    })?;
            }
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
            // Mastodon accepts both an embedded QuoteAuthorization and its bare URI in Delete.
            // Try the tightly actor-bound authorization lookup before interpreting a scalar URI
            // as a Note; a miss is side-effect free and falls through to ordinary deletion.
            let forwarding_activity = activity
                .get("signature")
                .is_some_and(|signature| !signature.is_null())
                .then_some(&activity);
            if writer
                .apply_remote_quote_authorization_delete(
                    source_account_id,
                    &actor_uri,
                    &object_uri,
                    forwarding_activity,
                )
                .await
                .map_err(|error| {
                    remote_note_write_failure(
                        &error,
                        "remote QuoteAuthorization Delete write failed",
                    )
                })?
            {
                return Ok(());
            }
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
                    config.origin.as_str(),
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
            actor_uri,
            follow_uri,
            target_uri,
            nested_actor_uri,
        } => {
            let quote_matched = writer
                .apply_remote_quote_decision(
                    source_account_id,
                    &actor_uri,
                    &follow_uri,
                    nested_actor_uri.as_deref(),
                    target_uri.as_deref(),
                    None,
                    None,
                    false,
                    config.origin.as_str(),
                    job.delivery_target_account_id,
                )
                .await
                .map_err(|error| {
                    remote_note_write_failure(&error, "remote quote Reject write failed")
                })?;
            if !quote_matched {
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
        | &WriteError::Forbidden
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

const POLL_EXPIRATION_REPAIR_PAGE_SIZE: i64 = 256;
const POLL_EXPIRATION_REPAIR_MAX_PAGES: usize = 4;
const POLL_EXPIRATION_REPAIR_MAX_CANDIDATES: usize = 25;
const POLL_EXPIRATION_REPAIR_WALL_TIME: StdDuration = StdDuration::from_secs(2);
const POLL_EXPIRATION_REPAIR_WORK_TIME: StdDuration = StdDuration::from_millis(1_500);
const POLL_EXPIRATION_REPAIR_CONTINUATION_RESERVE: StdDuration = StdDuration::from_millis(500);
const POLL_EXPIRATION_REPAIR_OPERATION_TIMEOUT: StdDuration = StdDuration::from_secs(1);
const POLL_EXPIRATION_STARTUP_LEASE_DURATION: Duration = Duration::seconds(3);
const POLL_EXPIRATION_STARTUP_SUCCESS_POLL_INTERVAL: StdDuration = StdDuration::from_millis(25);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PollExpirationScanMode {
    Optimized,
    Raw,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PollExpirationScanStep {
    AdvanceTerminal,
    Reconcile,
    Stop,
}

fn poll_expiration_scan_step(
    terminal: bool,
    candidates_reconciled: usize,
) -> PollExpirationScanStep {
    if terminal {
        PollExpirationScanStep::AdvanceTerminal
    } else if candidates_reconciled >= POLL_EXPIRATION_REPAIR_MAX_CANDIDATES {
        PollExpirationScanStep::Stop
    } else {
        PollExpirationScanStep::Reconcile
    }
}

async fn within_poll_expiration_reconciliation_wall_time<F>(
    limit: StdDuration,
    future: F,
) -> Result<(), HandlerFailure>
where
    F: Future<Output = Result<(), HandlerFailure>>,
{
    tokio::time::timeout(limit, future)
        .await
        .unwrap_or_else(|_| {
            Err(HandlerFailure::retry(
                "poll expiration reconciliation wall time exceeded",
            ))
        })
}

fn poll_expiration_reconciliation_job(
    scan_started_at: DateTime<Utc>,
    through_poll_id: Option<i64>,
    after_poll_id: i64,
    segment: i64,
) -> JobSpec {
    poll_expiration_reconciliation_job_with_mode(
        scan_started_at,
        through_poll_id,
        after_poll_id,
        segment,
        PollExpirationScanMode::Optimized,
    )
}

fn poll_expiration_raw_reconciliation_job(
    scan_started_at: DateTime<Utc>,
    through_poll_id: i64,
    after_poll_id: i64,
    segment: i64,
) -> JobSpec {
    poll_expiration_reconciliation_job_with_mode(
        scan_started_at,
        Some(through_poll_id),
        after_poll_id,
        segment,
        PollExpirationScanMode::Raw,
    )
}

fn poll_expiration_reconciliation_continuation(
    scan_started_at: DateTime<Utc>,
    through_poll_id: Option<i64>,
    after_poll_id: i64,
    segment: i64,
    mode: PollExpirationScanMode,
) -> Result<JobSpec, HandlerFailure> {
    let next_segment = segment
        .checked_add(1)
        .ok_or_else(|| HandlerFailure::permanent("poll expiration repair segment overflowed"))?;
    match mode {
        PollExpirationScanMode::Optimized => Ok(poll_expiration_reconciliation_job(
            scan_started_at,
            through_poll_id,
            after_poll_id,
            next_segment,
        )),
        PollExpirationScanMode::Raw => Ok(poll_expiration_raw_reconciliation_job(
            scan_started_at,
            through_poll_id.ok_or_else(|| {
                HandlerFailure::permanent("raw poll expiration repair high-water mark is missing")
            })?,
            after_poll_id,
            next_segment,
        )),
    }
}

async fn enqueue_poll_expiration_reconciliation_continuation(
    queue: &Queue,
    scan_started_at: DateTime<Utc>,
    through_poll_id: Option<i64>,
    after_poll_id: i64,
    segment: i64,
    mode: PollExpirationScanMode,
) -> Result<(), HandlerFailure> {
    let continuation = poll_expiration_reconciliation_continuation(
        scan_started_at,
        through_poll_id,
        after_poll_id,
        segment,
        mode,
    )?;
    queue
        .enqueue_exact(&continuation)
        .await
        .map_err(|_| HandlerFailure::retry("poll expiration repair continuation failed"))?;
    Ok(())
}

fn poll_expiration_reconciliation_job_with_mode(
    scan_started_at: DateTime<Utc>,
    through_poll_id: Option<i64>,
    after_poll_id: i64,
    segment: i64,
    mode: PollExpirationScanMode,
) -> JobSpec {
    let mut arguments = json!({
        "scan_started_at": scan_started_at.to_rfc3339(),
        "through_poll_id": through_poll_id,
        "after_poll_id": after_poll_id,
        "segment": segment,
    });
    if mode == PollExpirationScanMode::Raw {
        arguments["scan_mode"] = Value::String("raw".to_owned());
    }
    let logical_key =
        poll_expiration_reconciliation_logical_key(scan_started_at, after_poll_id, segment, mode);
    JobSpec::new(
        Lane::Maintenance,
        MASTODON_POLL_EXPIRATION_RECONCILE_JOB_KIND,
        arguments,
    )
    .logical_key(logical_key)
}

fn poll_expiration_reconciliation_logical_key(
    scan_started_at: DateTime<Utc>,
    after_poll_id: i64,
    segment: i64,
    mode: PollExpirationScanMode,
) -> String {
    match mode {
        PollExpirationScanMode::Optimized => format!(
            "poll-expiration-reconcile:{}:{segment}:{after_poll_id}",
            scan_started_at.timestamp_micros()
        ),
        PollExpirationScanMode::Raw => format!(
            "poll-expiration-reconcile:{}:raw:{segment}:{after_poll_id}",
            scan_started_at.timestamp_micros()
        ),
    }
}

fn parse_poll_expiration_scan_mode(
    arguments: &Value,
    segment: i64,
) -> Result<PollExpirationScanMode, HandlerFailure> {
    match arguments.get("scan_mode") {
        None => Ok(PollExpirationScanMode::Optimized),
        Some(Value::String(mode)) if mode == "raw" => {
            if segment == 0
                || arguments
                    .get("through_poll_id")
                    .and_then(Value::as_i64)
                    .is_none()
                || arguments
                    .get("after_poll_id")
                    .and_then(Value::as_i64)
                    .is_none()
            {
                return Err(HandlerFailure::permanent(
                    "raw poll expiration repair continuation is invalid",
                ));
            }
            Ok(PollExpirationScanMode::Raw)
        }
        Some(_) => Err(HandlerFailure::permanent(
            "poll expiration repair scan mode is invalid",
        )),
    }
}

fn poll_expiration_no_progress() -> HandlerFailure {
    HandlerFailure::retry("poll expiration reconciliation made no cursor progress")
}

fn poll_expiration_execution_mode(
    configured_mode: PollExpirationScanMode,
    attempt: i32,
) -> PollExpirationScanMode {
    if configured_mode == PollExpirationScanMode::Raw || attempt > 1 {
        PollExpirationScanMode::Raw
    } else {
        PollExpirationScanMode::Optimized
    }
}

fn poll_expiration_reconciliation_key_from_arguments(
    arguments: &Value,
) -> Result<String, HandlerFailure> {
    let scan_started_at = arguments
        .get("scan_started_at")
        .or_else(|| arguments.get("cutoff"))
        .and_then(Value::as_str)
        .ok_or_else(|| HandlerFailure::permanent("poll expiration repair start is missing"))?
        .parse::<DateTime<Utc>>()
        .map_err(|_| HandlerFailure::permanent("poll expiration repair start is invalid"))?;
    let after_poll_id = match arguments.get("after_poll_id") {
        None => 0,
        Some(value) => value
            .as_i64()
            .ok_or_else(|| HandlerFailure::permanent("poll expiration repair cursor is invalid"))?,
    };
    let segment = match arguments.get("segment") {
        None => 0,
        Some(value) => value.as_i64().ok_or_else(|| {
            HandlerFailure::permanent("poll expiration repair segment is invalid")
        })?,
    };
    let mode = parse_poll_expiration_scan_mode(arguments, segment)?;
    Ok(poll_expiration_reconciliation_logical_key(
        scan_started_at,
        after_poll_id,
        segment,
        mode,
    ))
}

fn poll_expiration_scan_timed_out(error: &sqlx::Error) -> bool {
    matches!(
        error,
        sqlx::Error::Database(database) if database.code().as_deref() == Some("57014")
    )
}

type PollExpirationScanRow = (
    i64,
    NaiveDateTime,
    bool,
    Option<(Value, Option<DateTime<Utc>>)>,
);

#[derive(Debug)]
enum PollExpirationHighWater {
    Value(i64),
    TimedOut,
}

async fn poll_expiration_high_water(
    writer_pool: &PgPool,
    work_deadline: tokio::time::Instant,
) -> Result<PollExpirationHighWater, HandlerFailure> {
    let mut transaction = writer_pool
        .begin()
        .await
        .map_err(|_| HandlerFailure::retry("poll expiration repair transaction failed"))?;
    let statement_timeout = work_deadline
        .saturating_duration_since(tokio::time::Instant::now())
        .min(POLL_EXPIRATION_REPAIR_OPERATION_TIMEOUT);
    if statement_timeout.is_zero() {
        transaction
            .rollback()
            .await
            .map_err(|_| HandlerFailure::retry("poll expiration high-water rollback failed"))?;
        return Ok(PollExpirationHighWater::TimedOut);
    }
    sqlx::query("SELECT set_config('statement_timeout', $1, true)")
        .bind(poll_expiration_statement_timeout_value(statement_timeout))
        .execute(&mut *transaction)
        .await
        .map_err(|_| {
            HandlerFailure::retry("poll expiration repair statement timeout setup failed")
        })?;
    let result = sqlx::query_scalar::<_, Option<i64>>("SELECT max(id) FROM polls")
        .fetch_one(&mut *transaction)
        .await;
    match result {
        Ok(through) => {
            transaction
                .commit()
                .await
                .map_err(|_| HandlerFailure::retry("poll expiration repair scan commit failed"))?;
            Ok(PollExpirationHighWater::Value(through.unwrap_or(0)))
        }
        Err(error) if poll_expiration_scan_timed_out(&error) => {
            transaction
                .rollback()
                .await
                .map_err(|_| HandlerFailure::retry("poll expiration high-water rollback failed"))?;
            Ok(PollExpirationHighWater::TimedOut)
        }
        Err(_) => {
            transaction
                .rollback()
                .await
                .map_err(|_| HandlerFailure::retry("poll expiration high-water rollback failed"))?;
            Err(HandlerFailure::retry(
                "poll expiration repair high-water scan failed",
            ))
        }
    }
}

#[derive(Debug)]
enum PollExpirationOptimizedPage {
    Rows(Vec<PollExpirationScanRow>),
    TimedOut,
}

fn poll_expiration_statement_timeout_value(limit: StdDuration) -> String {
    format!("{}ms", limit.as_millis().max(1))
}

#[allow(clippy::too_many_lines)]
async fn poll_expiration_optimized_page(
    writer_pool: &PgPool,
    cursor: i64,
    through_poll_id: i64,
    work_deadline: tokio::time::Instant,
    force_timeout: bool,
) -> Result<PollExpirationOptimizedPage, HandlerFailure> {
    let mut transaction = writer_pool
        .begin()
        .await
        .map_err(|_| HandlerFailure::retry("poll expiration repair transaction failed"))?;
    sqlx::query("SET LOCAL lock_timeout = '1s'")
        .execute(&mut *transaction)
        .await
        .map_err(|_| HandlerFailure::retry("poll expiration repair lock timeout setup failed"))?;
    let statement_timeout = work_deadline
        .saturating_duration_since(tokio::time::Instant::now())
        .min(POLL_EXPIRATION_REPAIR_OPERATION_TIMEOUT);
    if statement_timeout.is_zero() {
        transaction
            .rollback()
            .await
            .map_err(|_| HandlerFailure::retry("poll expiration timed-out scan rollback failed"))?;
        return Ok(PollExpirationOptimizedPage::TimedOut);
    }
    sqlx::query("SELECT set_config('statement_timeout', $1, true)")
        .bind(poll_expiration_statement_timeout_value(statement_timeout))
        .execute(&mut *transaction)
        .await
        .map_err(|_| {
            HandlerFailure::retry("poll expiration repair statement timeout setup failed")
        })?;
    let candidate_query = sqlx::query_as::<_, (i64, NaiveDateTime, bool)>(
        "SELECT poll.id, poll.expires_at, author.domain IS NULL \
           FROM polls poll \
           JOIN statuses status ON status.id = poll.status_id \
           JOIN accounts author ON author.id = poll.account_id \
          WHERE poll.id > $1 AND poll.id <= $2 \
            AND status.deleted_at IS NULL AND poll.expires_at IS NOT NULL \
            AND (author.domain IS NULL OR EXISTS ( \
                SELECT 1 FROM poll_votes vote \
                JOIN accounts voter ON voter.id = vote.account_id \
                WHERE vote.poll_id = poll.id AND voter.domain IS NULL)) \
            AND NOT EXISTS ( \
                SELECT 1 FROM rustodon.outbox_events terminal \
                WHERE terminal.kind = $4 \
                  AND terminal.logical_key = format( \
                    'poll-expiration-effect:%s:generation:%s', poll.id, \
                    (extract(epoch FROM poll.expires_at) * 1000000)::bigint) \
                  AND terminal.dispatched_at IS NOT NULL \
                  AND terminal.payload IN ( \
                    jsonb_build_object( \
                      'version', 1, 'poll_id', poll.id, \
                      'expires_at_micros', \
                        (extract(epoch FROM poll.expires_at) * 1000000)::bigint, \
                      'outcome', 'historical_baseline'), \
                    jsonb_build_object( \
                      'version', 1, 'poll_id', poll.id, \
                      'expires_at_micros', \
                        (extract(epoch FROM poll.expires_at) * 1000000)::bigint, \
                      'outcome', 'effects_enqueued'), \
                    jsonb_build_object( \
                      'version', 1, 'poll_id', poll.id, \
                      'expires_at_micros', \
                        (extract(epoch FROM poll.expires_at) * 1000000)::bigint, \
                      'outcome', 'remote_past_expiry_suppressed'))) \
          ORDER BY poll.id LIMIT $3",
    )
    .bind(cursor)
    .bind(through_poll_id)
    .bind(POLL_EXPIRATION_REPAIR_PAGE_SIZE)
    .bind(MASTODON_POLL_EXPIRATION_EFFECT_KIND);
    #[cfg(feature = "test-support")]
    let forced_error = if force_timeout {
        sqlx::query(
            "DO $$ BEGIN \
               RAISE EXCEPTION USING ERRCODE = '57014', \
                 MESSAGE = 'forced poll expiration scan timeout'; \
             END $$",
        )
        .execute(&mut *transaction)
        .await
        .err()
    } else {
        None
    };
    #[cfg(not(feature = "test-support"))]
    let forced_error = {
        let _ = force_timeout;
        None
    };
    let rows_result = if let Some(error) = forced_error {
        Err(error)
    } else {
        candidate_query.fetch_all(&mut *transaction).await
    };
    match rows_result {
        Ok(rows) => {
            transaction
                .commit()
                .await
                .map_err(|_| HandlerFailure::retry("poll expiration repair scan commit failed"))?;
            Ok(PollExpirationOptimizedPage::Rows(
                rows.into_iter()
                    .map(|(poll_id, expires_at, local_author)| {
                        (poll_id, expires_at, local_author, None)
                    })
                    .collect(),
            ))
        }
        Err(error) if poll_expiration_scan_timed_out(&error) => {
            transaction.rollback().await.map_err(|_| {
                HandlerFailure::retry("poll expiration timed-out scan rollback failed")
            })?;
            Ok(PollExpirationOptimizedPage::TimedOut)
        }
        Err(_) => {
            transaction.rollback().await.map_err(|_| {
                HandlerFailure::retry("poll expiration failed scan rollback failed")
            })?;
            Err(HandlerFailure::retry("poll expiration repair scan failed"))
        }
    }
}

#[derive(Debug)]
enum PollExpirationFallbackPageError {
    TimedOut,
    Failure(HandlerFailure),
}

#[allow(clippy::too_many_lines)]
async fn poll_expiration_fallback_page(
    writer_pool: &PgPool,
    cursor: i64,
    through_poll_id: i64,
    work_deadline: tokio::time::Instant,
) -> Result<Vec<PollExpirationScanRow>, PollExpirationFallbackPageError> {
    let mut transaction = writer_pool.begin().await.map_err(|_| {
        PollExpirationFallbackPageError::Failure(HandlerFailure::retry(
            "poll expiration fallback transaction failed",
        ))
    })?;
    sqlx::query("SET LOCAL lock_timeout = '1s'")
        .execute(&mut *transaction)
        .await
        .map_err(|_| {
            PollExpirationFallbackPageError::Failure(HandlerFailure::retry(
                "poll expiration fallback lock timeout setup failed",
            ))
        })?;
    let statement_timeout = work_deadline
        .saturating_duration_since(tokio::time::Instant::now())
        .min(POLL_EXPIRATION_REPAIR_OPERATION_TIMEOUT);
    if statement_timeout.is_zero() {
        transaction.rollback().await.map_err(|_| {
            PollExpirationFallbackPageError::Failure(HandlerFailure::retry(
                "poll expiration fallback scan rollback failed",
            ))
        })?;
        return Err(PollExpirationFallbackPageError::TimedOut);
    }
    sqlx::query("SELECT set_config('statement_timeout', $1, true)")
        .bind(poll_expiration_statement_timeout_value(statement_timeout))
        .execute(&mut *transaction)
        .await
        .map_err(|_| {
            PollExpirationFallbackPageError::Failure(HandlerFailure::retry(
                "poll expiration fallback statement timeout setup failed",
            ))
        })?;
    let rows_result = sqlx::query_as::<_, (i64, NaiveDateTime, bool)>(
        "SELECT poll.id, poll.expires_at, author.domain IS NULL \
           FROM polls poll \
           JOIN statuses status ON status.id = poll.status_id \
           JOIN accounts author ON author.id = poll.account_id \
          WHERE poll.id > $1 AND poll.id <= $2 \
            AND status.deleted_at IS NULL AND poll.expires_at IS NOT NULL \
            AND (author.domain IS NULL OR EXISTS ( \
                SELECT 1 FROM poll_votes vote \
                JOIN accounts voter ON voter.id = vote.account_id \
                WHERE vote.poll_id = poll.id AND voter.domain IS NULL)) \
          ORDER BY poll.id LIMIT $3",
    )
    .bind(cursor)
    .bind(through_poll_id)
    .bind(POLL_EXPIRATION_REPAIR_PAGE_SIZE)
    .fetch_all(&mut *transaction)
    .await;
    let rows = match rows_result {
        Ok(rows) => rows,
        Err(error) => {
            transaction.rollback().await.map_err(|_| {
                PollExpirationFallbackPageError::Failure(HandlerFailure::retry(
                    "poll expiration fallback scan rollback failed",
                ))
            })?;
            if poll_expiration_scan_timed_out(&error) {
                return Err(PollExpirationFallbackPageError::TimedOut);
            }
            return Err(PollExpirationFallbackPageError::Failure(
                HandlerFailure::retry("poll expiration fallback scan failed"),
            ));
        }
    };
    let keys = rows
        .iter()
        .map(|(poll_id, expires_at, _)| {
            poll_expiration_effect_key(*poll_id, poll_expiration_generation(expires_at.and_utc()))
        })
        .collect::<Vec<_>>();
    let markers_result = if keys.is_empty() {
        Ok(Vec::new())
    } else {
        let statement_timeout = work_deadline
            .saturating_duration_since(tokio::time::Instant::now())
            .min(POLL_EXPIRATION_REPAIR_OPERATION_TIMEOUT);
        if statement_timeout.is_zero() {
            transaction.rollback().await.map_err(|_| {
                PollExpirationFallbackPageError::Failure(HandlerFailure::retry(
                    "poll expiration fallback marker rollback failed",
                ))
            })?;
            return Err(PollExpirationFallbackPageError::TimedOut);
        }
        sqlx::query("SELECT set_config('statement_timeout', $1, true)")
            .bind(poll_expiration_statement_timeout_value(statement_timeout))
            .execute(&mut *transaction)
            .await
            .map_err(|_| {
                PollExpirationFallbackPageError::Failure(HandlerFailure::retry(
                    "poll expiration fallback marker timeout setup failed",
                ))
            })?;
        sqlx::query_as::<_, (String, Value, Option<DateTime<Utc>>)>(
            "SELECT logical_key, payload, dispatched_at \
               FROM rustodon.outbox_events \
              WHERE kind = $1 AND logical_key = ANY($2)",
        )
        .bind(MASTODON_POLL_EXPIRATION_EFFECT_KIND)
        .bind(&keys)
        .fetch_all(&mut *transaction)
        .await
    };
    let markers = match markers_result {
        Ok(markers) => markers,
        Err(error) => {
            transaction.rollback().await.map_err(|_| {
                PollExpirationFallbackPageError::Failure(HandlerFailure::retry(
                    "poll expiration fallback marker rollback failed",
                ))
            })?;
            if poll_expiration_scan_timed_out(&error) {
                return Err(PollExpirationFallbackPageError::TimedOut);
            }
            return Err(PollExpirationFallbackPageError::Failure(
                HandlerFailure::retry("poll expiration fallback marker scan failed"),
            ));
        }
    };
    let mut markers = markers
        .into_iter()
        .map(|(key, payload, dispatched_at)| (key, (payload, dispatched_at)))
        .collect::<BTreeMap<_, _>>();
    let classified = rows
        .into_iter()
        .zip(keys)
        .map(|((poll_id, expires_at, local_author), key)| {
            (poll_id, expires_at, local_author, markers.remove(&key))
        })
        .collect();
    transaction.commit().await.map_err(|_| {
        PollExpirationFallbackPageError::Failure(HandlerFailure::retry(
            "poll expiration fallback scan commit failed",
        ))
    })?;
    Ok(classified)
}

async fn poll_expiration_fallback_page_within_work_time(
    writer_pool: &PgPool,
    cursor: i64,
    through_poll_id: i64,
    started: tokio::time::Instant,
) -> Result<Option<Vec<PollExpirationScanRow>>, HandlerFailure> {
    let remaining = POLL_EXPIRATION_REPAIR_WORK_TIME.saturating_sub(started.elapsed());
    if remaining.is_zero() {
        return Ok(None);
    }
    let work_deadline = started + POLL_EXPIRATION_REPAIR_WORK_TIME;
    match tokio::time::timeout(
        remaining,
        poll_expiration_fallback_page(writer_pool, cursor, through_poll_id, work_deadline),
    )
    .await
    {
        Ok(Ok(rows)) => Ok(Some(rows)),
        Ok(Err(PollExpirationFallbackPageError::TimedOut)) | Err(_) => Ok(None),
        Ok(Err(PollExpirationFallbackPageError::Failure(error))) => Err(error),
    }
}

async fn reconcile_poll_expirations(
    queue: Queue,
    writer_pool: PgPool,
    arguments: Value,
    logical_key: String,
    attempt: i32,
    wall_deadline: tokio::time::Instant,
) -> Result<(), HandlerFailure> {
    let expected_key = poll_expiration_reconciliation_key_from_arguments(&arguments)?;
    if logical_key != expected_key {
        return Err(HandlerFailure::permanent(
            "poll expiration reconciliation logical key is invalid",
        ));
    }
    let now = tokio::time::Instant::now();
    let remaining_wall = wall_deadline.saturating_duration_since(now);
    let work_allowance = remaining_wall
        .saturating_sub(POLL_EXPIRATION_REPAIR_CONTINUATION_RESERVE)
        .min(POLL_EXPIRATION_REPAIR_WORK_TIME);
    let elapsed_work = POLL_EXPIRATION_REPAIR_WORK_TIME.saturating_sub(work_allowance);
    let started = now.checked_sub(elapsed_work).unwrap_or(now);
    within_poll_expiration_reconciliation_wall_time(remaining_wall, async move {
        reconcile_poll_expirations_inner(
            queue,
            writer_pool,
            arguments,
            attempt,
            started,
            false,
            false,
        )
        .await
    })
    .await
}

#[cfg(feature = "test-support")]
/// Runs one reconciliation segment while forcing the optimized scan to return `PostgreSQL`
/// SQLSTATE 57014, the statement-timeout/query-cancellation code. This is only exposed to
/// disposable worker fixtures.
///
/// # Errors
///
/// Returns an error when bounded fallback or continuation persistence fails.
pub async fn reconcile_poll_expirations_with_primary_timeout_for_test(
    queue: Queue,
    writer_pool: PgPool,
    arguments: Value,
    attempt: i32,
) -> Result<(), HandlerFailure> {
    let started = tokio::time::Instant::now();
    within_poll_expiration_reconciliation_wall_time(POLL_EXPIRATION_REPAIR_WALL_TIME, async move {
        reconcile_poll_expirations_inner(
            queue,
            writer_pool,
            arguments,
            attempt,
            started,
            true,
            false,
        )
        .await
    })
    .await
}

#[cfg(feature = "test-support")]
/// Forces SQLSTATE 57014 and then simulates a fallback timeout without cursor progress.
///
/// # Errors
///
/// Returns a retryable error when neither scan strategy advances the cursor.
pub async fn reconcile_poll_expirations_with_exhausted_primary_timeout_for_test(
    queue: Queue,
    writer_pool: PgPool,
    arguments: Value,
    attempt: i32,
) -> Result<(), HandlerFailure> {
    let started = tokio::time::Instant::now();
    within_poll_expiration_reconciliation_wall_time(POLL_EXPIRATION_REPAIR_WALL_TIME, async move {
        reconcile_poll_expirations_inner(
            queue,
            writer_pool,
            arguments,
            attempt,
            started,
            true,
            true,
        )
        .await
    })
    .await
}

#[cfg(feature = "test-support")]
/// Simulates a raw scan timeout without cursor progress.
///
/// # Errors
///
/// Returns an error for non-raw arguments or a simulated raw timeout.
pub async fn reconcile_poll_expirations_with_exhausted_raw_budget_for_test(
    queue: Queue,
    writer_pool: PgPool,
    arguments: Value,
    attempt: i32,
) -> Result<(), HandlerFailure> {
    let segment = arguments
        .get("segment")
        .and_then(Value::as_i64)
        .ok_or_else(|| HandlerFailure::permanent("poll expiration repair segment is invalid"))?;
    if parse_poll_expiration_scan_mode(&arguments, segment)? != PollExpirationScanMode::Raw {
        return Err(HandlerFailure::permanent(
            "poll expiration repair test continuation is not raw",
        ));
    }
    let started = tokio::time::Instant::now();
    within_poll_expiration_reconciliation_wall_time(POLL_EXPIRATION_REPAIR_WALL_TIME, async move {
        reconcile_poll_expirations_inner(
            queue,
            writer_pool,
            arguments,
            attempt,
            started,
            false,
            true,
        )
        .await
    })
    .await
}

#[allow(clippy::too_many_lines)]
async fn reconcile_poll_expirations_inner(
    queue: Queue,
    writer_pool: PgPool,
    arguments: Value,
    attempt: i32,
    started: tokio::time::Instant,
    force_primary_timeout: bool,
    force_fallback_timeout: bool,
) -> Result<(), HandlerFailure> {
    let scan_started_at = arguments
        .get("scan_started_at")
        .or_else(|| arguments.get("cutoff"))
        .and_then(Value::as_str)
        .ok_or_else(|| HandlerFailure::permanent("poll expiration repair start is missing"))?
        .parse::<DateTime<Utc>>()
        .map_err(|_| HandlerFailure::permanent("poll expiration repair start is invalid"))?;
    let after_poll_id = match arguments.get("after_poll_id") {
        None => 0,
        Some(value) => value
            .as_i64()
            .ok_or_else(|| HandlerFailure::permanent("poll expiration repair cursor is invalid"))?,
    };
    if after_poll_id < 0 {
        return Err(HandlerFailure::permanent(
            "poll expiration repair cursor is invalid",
        ));
    }
    let segment = match arguments.get("segment") {
        None => 0,
        Some(value) => value.as_i64().ok_or_else(|| {
            HandlerFailure::permanent("poll expiration repair segment is invalid")
        })?,
    };
    if segment < 0 {
        return Err(HandlerFailure::permanent(
            "poll expiration repair segment is invalid",
        ));
    }
    let scan_mode = poll_expiration_execution_mode(
        parse_poll_expiration_scan_mode(&arguments, segment)?,
        attempt,
    );
    let initial_cursor = after_poll_id;
    let through_poll_id = match arguments.get("through_poll_id") {
        Some(Value::Null) | None => {
            let remaining = POLL_EXPIRATION_REPAIR_WORK_TIME.saturating_sub(started.elapsed());
            let high_water = if remaining.is_zero() {
                PollExpirationHighWater::TimedOut
            } else {
                match tokio::time::timeout(
                    remaining.min(POLL_EXPIRATION_REPAIR_OPERATION_TIMEOUT),
                    poll_expiration_high_water(
                        &writer_pool,
                        started + POLL_EXPIRATION_REPAIR_WORK_TIME,
                    ),
                )
                .await
                {
                    Ok(result) => result?,
                    Err(_) => PollExpirationHighWater::TimedOut,
                }
            };
            match high_water {
                PollExpirationHighWater::Value(through) => through,
                PollExpirationHighWater::TimedOut => {
                    return Err(poll_expiration_no_progress());
                }
            }
        }
        Some(value) => value.as_i64().ok_or_else(|| {
            HandlerFailure::permanent("poll expiration repair high-water mark is invalid")
        })?,
    };
    if through_poll_id < after_poll_id || through_poll_id < 0 {
        return Err(HandlerFailure::permanent(
            "poll expiration repair high-water mark is invalid",
        ));
    }
    if through_poll_id == after_poll_id {
        return Ok(());
    }
    if started.elapsed() >= POLL_EXPIRATION_REPAIR_WORK_TIME {
        return Err(poll_expiration_no_progress());
    }
    let remaining = POLL_EXPIRATION_REPAIR_WORK_TIME.saturating_sub(started.elapsed());
    let activation = if let Ok(result) =
        tokio::time::timeout(remaining, queue.poll_expiration_activation()).await
    {
        result.map_err(|_| {
            HandlerFailure::permanent("poll expiration activation marker is invalid")
        })?
    } else {
        return Err(poll_expiration_no_progress());
    };

    let mut cursor = after_poll_id;
    let mut candidates_reconciled = 0_usize;
    let mut pages_scanned = 0_usize;
    let mut bounded = false;
    let mut exhausted = false;
    let mut force_primary_timeout = force_primary_timeout;

    'pages: while pages_scanned < POLL_EXPIRATION_REPAIR_MAX_PAGES && cursor < through_poll_id {
        if started.elapsed() >= POLL_EXPIRATION_REPAIR_WORK_TIME {
            bounded = true;
            break;
        }
        let (rows, fallback_page) = if scan_mode == PollExpirationScanMode::Raw {
            let fallback_rows = if force_fallback_timeout {
                None
            } else {
                poll_expiration_fallback_page_within_work_time(
                    &writer_pool,
                    cursor,
                    through_poll_id,
                    started,
                )
                .await?
            };
            let Some(rows) = fallback_rows else {
                bounded = true;
                break;
            };
            (rows, true)
        } else {
            let remaining = POLL_EXPIRATION_REPAIR_WORK_TIME.saturating_sub(started.elapsed());
            let force_timeout = force_primary_timeout;
            force_primary_timeout = false;
            let optimized_page = if let Ok(result) = tokio::time::timeout(
                remaining,
                poll_expiration_optimized_page(
                    &writer_pool,
                    cursor,
                    through_poll_id,
                    started + POLL_EXPIRATION_REPAIR_WORK_TIME,
                    force_timeout,
                ),
            )
            .await
            {
                result?
            } else {
                bounded = true;
                break;
            };
            match optimized_page {
                PollExpirationOptimizedPage::Rows(rows) => (rows, false),
                PollExpirationOptimizedPage::TimedOut => {
                    let fallback_rows = if force_fallback_timeout {
                        None
                    } else {
                        poll_expiration_fallback_page_within_work_time(
                            &writer_pool,
                            cursor,
                            through_poll_id,
                            started,
                        )
                        .await?
                    };
                    let Some(rows) = fallback_rows else {
                        bounded = true;
                        break;
                    };
                    (rows, true)
                }
            }
        };
        pages_scanned += 1;
        if rows.is_empty() {
            exhausted = true;
            break;
        }
        let short_page =
            rows.len() < usize::try_from(POLL_EXPIRATION_REPAIR_PAGE_SIZE).unwrap_or(usize::MAX);
        for (poll_id, expires_at, local_author, marker) in rows {
            if started.elapsed() >= POLL_EXPIRATION_REPAIR_WORK_TIME {
                bounded = true;
                break 'pages;
            }
            let terminal = if let Some((payload, dispatched_at)) = marker {
                validate_dispatched_poll_expiration_effect(
                    &payload,
                    dispatched_at,
                    poll_id,
                    poll_expiration_generation(expires_at.and_utc()),
                )
                .map_err(|_| HandlerFailure::retry("poll expiration fallback marker is invalid"))?;
                true
            } else {
                false
            };
            match poll_expiration_scan_step(terminal, candidates_reconciled) {
                PollExpirationScanStep::AdvanceTerminal => {
                    cursor = poll_id;
                    continue;
                }
                PollExpirationScanStep::Stop => {
                    bounded = true;
                    break 'pages;
                }
                PollExpirationScanStep::Reconcile => {}
            }
            let expires_at = expires_at.and_utc();
            let run_at = expires_at
                + if local_author {
                    Duration::zero()
                } else {
                    Duration::minutes(5)
                };
            let remaining = POLL_EXPIRATION_REPAIR_WORK_TIME.saturating_sub(started.elapsed());
            let operation_timeout = remaining.min(POLL_EXPIRATION_REPAIR_OPERATION_TIMEOUT);
            match tokio::time::timeout(
                operation_timeout,
                queue.reconcile_poll_expiration(poll_id, expires_at, run_at, activation),
            )
            .await
            {
                Ok(Ok(_)) => {
                    candidates_reconciled += 1;
                    cursor = poll_id;
                }
                Ok(Err(_)) => {
                    return Err(HandlerFailure::retry(
                        "poll expiration durable repair failed",
                    ));
                }
                Err(_) => {
                    bounded = true;
                    break 'pages;
                }
            }
        }
        if fallback_page {
            if short_page {
                exhausted = true;
            } else {
                bounded = true;
            }
            break;
        }
        if short_page {
            exhausted = true;
            break;
        }
    }
    if !exhausted && cursor < through_poll_id {
        bounded = true;
    }
    if bounded && cursor < through_poll_id {
        if cursor == initial_cursor {
            return Err(poll_expiration_no_progress());
        }
        enqueue_poll_expiration_reconciliation_continuation(
            &queue,
            scan_started_at,
            Some(through_poll_id),
            cursor,
            segment,
            scan_mode,
        )
        .await?;
    }
    Ok(())
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
    let activity_writer = mastodon_writer.clone();
    let report_mail_enabled = mail_runtime.is_some();
    let domain_block_media_root = federation
        .as_ref()
        .and_then(|config| config.media_root.clone());
    if let Some(mastodon_writer) = mastodon_writer {
        if let Some(media_root) = domain_block_media_root.clone() {
            local_uploads::register(
                &handlers,
                mastodon_writer.clone(),
                queue.clone(),
                media_root.clone(),
            )?;
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
        let poll_expiration_pool = mastodon_writer.clone();
        handlers.register(
            MASTODON_POLL_EXPIRATION_JOB_KIND,
            Lane::Core,
            ResourceClass::None,
            move |job| {
                let pool = poll_expiration_pool.clone();
                async move {
                    let Some(poll_id) = job.arguments.get("poll_id").and_then(Value::as_i64) else {
                        return Err(HandlerFailure::permanent(
                            "poll expiration job is missing its poll ID",
                        ));
                    };
                    if poll_id <= 0 {
                        return Err(HandlerFailure::permanent(
                            "poll expiration job poll ID is invalid",
                        ));
                    }
                    let expires_at_micros = job
                        .arguments
                        .get("expires_at_micros")
                        .and_then(Value::as_i64)
                        .ok_or_else(|| {
                            HandlerFailure::permanent(
                                "poll expiration job generation is missing or invalid",
                            )
                        })?;
                    WriteRepository::from_pool(pool)
                        .expire_poll_generation(poll_id, Some(expires_at_micros))
                        .await
                        .map_err(|error| match error {
                            WriteError::InvalidInput(_)
                            | WriteError::Job(JobError::InvalidData(_)) => {
                                HandlerFailure::permanent(
                                    "poll expiration safety marker is invalid",
                                )
                            }
                            _ => HandlerFailure::retry("poll expiration processing failed"),
                        })
                }
            },
        )?;
        let poll_expiration_repair_pool = mastodon_writer.clone();
        let poll_expiration_repair_queue = queue.clone();
        handlers.register(
            MASTODON_POLL_EXPIRATION_RECONCILE_JOB_KIND,
            Lane::Maintenance,
            ResourceClass::None,
            move |job| {
                let pool = poll_expiration_repair_pool.clone();
                let queue = poll_expiration_repair_queue.clone();
                Box::pin(async move {
                    let Some(logical_key) = job.logical_key.clone() else {
                        return Err(HandlerFailure::permanent(
                            "poll expiration reconciliation logical key is missing",
                        ));
                    };
                    let arguments = job.arguments.clone();
                    let attempt = job.attempt;
                    reconcile_poll_expirations(
                        queue,
                        pool,
                        arguments,
                        logical_key,
                        attempt,
                        tokio::time::Instant::now() + POLL_EXPIRATION_REPAIR_WALL_TIME,
                    )
                    .await
                }) as HandlerFuture
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
            #[cfg(feature = "test-support")]
            let thread_fetcher =
                thread_fetcher.with_test_endpoint(federation.remote_fetch_endpoint);
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
                let profile_pool = mastodon_writer.clone();
                let profile_queue = queue.clone();
                let profile_config = federation.clone();
                let profile_fetcher = remote_fetcher.clone();
                let profile_root = media_root.clone();
                handlers.register(
                    ACTIVITYPUB_PROFILE_MEDIA_FETCH_JOB_KIND,
                    Lane::Pull,
                    ResourceClass::Media,
                    move |job| {
                        let pool = profile_pool.clone();
                        let queue = profile_queue.clone();
                        let config = profile_config.clone();
                        let fetcher = profile_fetcher.clone();
                        let root = profile_root.clone();
                        async move {
                            profile_media::fetch_profile_image(
                                pool,
                                queue,
                                &config,
                                &fetcher,
                                root,
                                &job.arguments,
                            )
                            .await
                        }
                    },
                )?;
                let profile_pool = mastodon_writer.clone();
                let profile_root = media_root.clone();
                handlers.register(
                    ACTIVITYPUB_PROFILE_MEDIA_CLEANUP_JOB_KIND,
                    Lane::Maintenance,
                    ResourceClass::Media,
                    move |job| {
                        let pool = profile_pool.clone();
                        let root = profile_root.clone();
                        async move {
                            profile_media::cleanup_profile_images(pool, root, &job.arguments).await
                        }
                    },
                )?;

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
                            &job,
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
                        let poll_updated_at_micros = job
                            .arguments
                            .get("poll_updated_at_micros")
                            .and_then(Value::as_i64);
                        let quote_updated_at_micros = job
                            .arguments
                            .get("quote_updated_at_micros")
                            .and_then(Value::as_i64);
                        let update_kind = job.arguments.get("update_kind").and_then(Value::as_str);
                        let update_version_micros = job
                            .arguments
                            .get("update_version_micros")
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
                        let quote_request = if activity_type == "QuoteRequest" {
                            Some(QuoteRequestDistribution {
                                quote_id: job
                                    .arguments
                                    .get("quote_id")
                                    .and_then(Value::as_i64)
                                    .ok_or_else(|| {
                                        HandlerFailure::permanent(
                                            "quote-request distribution is missing its quote ID",
                                        )
                                    })?,
                                request_uri: job
                                    .arguments
                                    .get("quote_request_uri")
                                    .and_then(Value::as_str)
                                    .filter(|value| !value.is_empty())
                                    .ok_or_else(|| {
                                        HandlerFailure::permanent(
                                            "quote-request distribution is missing its request URI",
                                        )
                                    })?
                                    .to_owned(),
                                quoted_status_id: job
                                    .arguments
                                    .get("quoted_status_id")
                                    .and_then(Value::as_i64)
                                    .ok_or_else(|| {
                                        HandlerFailure::permanent(
                                            "quote-request distribution is missing its target status",
                                        )
                                    })?,
                                quoted_status_uri: job
                                    .arguments
                                    .get("quoted_status_uri")
                                    .and_then(Value::as_str)
                                    .filter(|value| !value.is_empty())
                                    .ok_or_else(|| {
                                        HandlerFailure::permanent(
                                            "quote-request distribution is missing its target URI",
                                        )
                                    })?
                                    .to_owned(),
                                quoted_status_url: job
                                    .arguments
                                    .get("quoted_status_url")
                                    .and_then(Value::as_str)
                                    .filter(|value| !value.is_empty())
                                    .ok_or_else(|| {
                                        HandlerFailure::permanent(
                                            "quote-request distribution is missing its target URL",
                                        )
                                    })?
                                    .to_owned(),
                                quoted_account_id: job
                                    .arguments
                                    .get("quoted_account_id")
                                    .and_then(Value::as_i64)
                                    .ok_or_else(|| {
                                        HandlerFailure::permanent(
                                            "quote-request distribution is missing its target account",
                                        )
                                    })?,
                            })
                        } else {
                            None
                        };
                        distribute_status(
                            pool,
                            &config,
                            status_id,
                            activity_type,
                            edited_at_micros,
                            poll_updated_at_micros,
                            quote_updated_at_micros,
                            update_kind,
                            update_version_micros,
                            &explicit_recipient_ids,
                            quote_request.as_ref(),
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
                        deliver_activity(pool, operational_pool, &config, &fetcher, &job).await
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
    let maintenance_queue = queue.clone();
    handlers.register(
        "rustodon.maintenance.prune",
        Lane::Maintenance,
        ResourceClass::None,
        move |_job| {
            let pool = pool.clone();
            let maintenance_queue = maintenance_queue.clone();
            let activity_writer = activity_writer.clone();
            async move {
                // Activity cleanup failure must not starve the operational cleanup below.
                let activity = match activity_writer {
                    Some(writer) => crate::activity::prune(&writer).await.map(drop),
                    None => Ok(()),
                };
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
                maintenance_queue
                    .prune_stream_history()
                    .await
                    .map_err(|_| HandlerFailure::retry("stream history cleanup failed"))?;
                activity.map_err(|_| HandlerFailure::retry("activity cleanup failed"))
            }
        },
    )?;
    Ok(handlers)
}

fn poll_expiration_startup_lease_owner(process_id: &str) -> String {
    let digest = Sha256::digest(process_id.as_bytes());
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    format!("poll-expiration-startup:{encoded}")
}

async fn reconcile_poll_expirations_at_startup(
    queue: Queue,
    writer_pool: PgPool,
    process_id: &str,
) -> Result<(), WorkerError> {
    let startup_deadline = tokio::time::Instant::now() + POLL_EXPIRATION_REPAIR_WALL_TIME;
    let lease_owner = poll_expiration_startup_lease_owner(process_id);
    let startup = async {
        queue.ensure_poll_expiration_activation().await?;
        let scan_started_at = Utc::now();
        let reservation = poll_expiration_reconciliation_job(scan_started_at, None, 0, 0)
            .run_at(scan_started_at + POLL_EXPIRATION_STARTUP_LEASE_DURATION);
        let reservation_key = reservation
            .logical_key_value()
            .expect("poll expiration reconciliation reservations have a logical key")
            .to_owned();
        let ownership = queue
            .enqueue_if_kind_idle(MASTODON_POLL_EXPIRATION_RECONCILE_JOB_KIND, &reservation)
            .await?;
        let job_id = ownership.job_id();
        let expected_arguments = ownership.arguments().clone();
        let logical_key = ownership
            .existing_logical_key()
            .unwrap_or(&reservation_key)
            .to_owned();
        loop {
            if queue
                .poll_expiration_reconciliation_succeeded(job_id, &logical_key, &expected_arguments)
                .await?
            {
                return Ok(());
            }
            match queue
                .claim_poll_expiration_reconciliation_for_startup(
                    job_id,
                    &logical_key,
                    &expected_arguments,
                    &lease_owner,
                    POLL_EXPIRATION_STARTUP_LEASE_DURATION,
                )
                .await?
            {
                PollExpirationStartupClaim::Claimed(job) => {
                    let job_logical_key = job
                        .logical_key
                        .clone()
                        .ok_or(WorkerError::StartupReconciliationFailed)?;
                    let result = reconcile_poll_expirations(
                        queue.clone(),
                        writer_pool.clone(),
                        job.arguments.clone(),
                        job_logical_key,
                        job.attempt,
                        startup_deadline,
                    )
                    .await;
                    if let Err(failure) = result {
                        transition_handler_failure(&queue, &job, &failure).await?;
                        return Err(WorkerError::StartupReconciliationFailed);
                    }
                    if queue
                        .complete_poll_expiration_reconciliation_success(&job)
                        .await?
                    {
                        return Ok(());
                    }
                    return Err(WorkerError::StartupReconciliationFailed);
                }
                PollExpirationStartupClaim::ActiveLease => {
                    tokio::time::sleep(POLL_EXPIRATION_STARTUP_SUCCESS_POLL_INTERVAL).await;
                }
                PollExpirationStartupClaim::Missing => {
                    if queue
                        .poll_expiration_reconciliation_succeeded(
                            job_id,
                            &logical_key,
                            &expected_arguments,
                        )
                        .await?
                    {
                        return Ok(());
                    }
                    return Err(WorkerError::StartupReconciliationFailed);
                }
            }
        }
    };
    tokio::time::timeout_at(startup_deadline, startup)
        .await
        .unwrap_or(Err(WorkerError::StartupReconciliationFailed))
}

/// Runs the bounded startup poll-expiration segment in disposable fixtures.
///
/// # Errors
///
/// Returns an error when activation or the bounded reconciliation segment cannot complete.
#[cfg(feature = "test-support")]
pub async fn reconcile_poll_expirations_at_startup_for_test(
    queue: Queue,
    writer_pool: PgPool,
    process_id: &str,
) -> Result<(), WorkerError> {
    reconcile_poll_expirations_at_startup(queue, writer_pool, process_id).await
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
    poll_expiration_writer: Option<PgPool>,
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
    let schedules_upload_recovery =
        schedules_maintenance && handlers.get(local_uploads::RECOVER_KIND)?.is_some();
    let executes_local_uploads =
        schedules_upload_recovery && handlers.get(local_uploads::PROCESS_KIND)?.is_some();
    let schedules_poll_expiration_repair = schedules_maintenance
        && handlers
            .get(MASTODON_POLL_EXPIRATION_RECONCILE_JOB_KIND)?
            .is_some();
    let executes_poll_expirations =
        lanes.contains(&Lane::Core) && handlers.get(MASTODON_POLL_EXPIRATION_JOB_KIND)?.is_some();
    if schedules_poll_expiration_repair {
        let writer_pool = poll_expiration_writer.ok_or(WorkerError::InvalidConfiguration(
            "poll expiration maintenance requires a writer pool",
        ))?;
        reconcile_poll_expirations_at_startup(queue.clone(), writer_pool, &process_id).await?;
    } else if executes_poll_expirations {
        // Core handlers establish the immutable boundary before executors or readiness heartbeats.
        tokio::time::timeout(
            POLL_EXPIRATION_REPAIR_WALL_TIME,
            queue.ensure_poll_expiration_activation(),
        )
        .await
        .map_err(|_| WorkerError::StartupReconciliationFailed)??;
    }
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
            json!({"concurrency": config.concurrency, "local_uploads": executes_local_uploads}),
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
        let mut maintenance_tick = tokio::time::interval_at(
            tokio::time::Instant::now() + StdDuration::from_mins(1),
            StdDuration::from_mins(1),
        );
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
                        json!({"concurrency": concurrency, "local_uploads": executes_local_uploads}),
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
                    let now = Utc::now();
                    let minute = now.timestamp() / 60;
                    scheduler_queue.enqueue(
                        &JobSpec::new(
                            Lane::Maintenance,
                            "rustodon.maintenance.prune",
                            json!({"minute": minute}),
                        ).logical_key(format!("maintenance:{minute}")),
                    ).await?;
                    if schedules_upload_recovery {
                        local_uploads::schedule_recovery(&scheduler_queue).await?;
                    }
                    if schedules_poll_expiration_repair {
                        scheduler_queue
                            .enqueue_if_kind_idle(
                                MASTODON_POLL_EXPIRATION_RECONCILE_JOB_KIND,
                                &poll_expiration_reconciliation_job(now, None, 0, 0),
                            )
                            .await?;
                    }
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
    use chrono::{DateTime, Duration, Utc};
    use http::StatusCode;
    use serde_json::json;

    use super::{
        FailureDisposition, HandlerRegistry, PollExpirationScanMode, PollExpirationScanStep,
        QuoteDeliveryKind, RemoteAnnounceTarget, ResourceClass, StatusUpdateKind,
        StatusUpdateVersionDecision, account_purge_cleanup_paths,
        account_update_delivery_logical_key, complete_status_update_delivery_kind,
        delivery_failure, delivery_logical_key, inbox_actor_domain,
        inferred_current_repair_delivery_kind, note_fetch_audience, note_resolution_logical_key,
        parse_poll_expiration_scan_mode, poll_expiration_execution_mode,
        poll_expiration_raw_reconciliation_job, poll_expiration_reconciliation_continuation,
        poll_expiration_reconciliation_job, poll_expiration_reconciliation_key_from_arguments,
        poll_expiration_scan_step, preferred_note_fetch_signer_id,
        quote_decision_allows_follow_fallback, quote_delivery_identity, quote_reference,
        quote_request_instrument_target_uri, quote_revision_is_current, quote_update_versions,
        remote_announce_document, remote_media_fetch_failure, remote_note_document,
        remote_note_fetch_failure, remote_note_quote_fetch_references,
        remote_poll_vote_allows_note_fallback, remote_quote_request_fetch_failure,
        resolved_create_note, retry_delay, safe_cleanup_path, status_snapshot_repair_activity_id,
        status_update_delivery_is_current, status_update_version_decision, status_update_versions,
        update_delivery_is_current, update_delivery_logical_key, validate_create_binding,
        within_poll_expiration_reconciliation_wall_time,
    };
    use crate::jobs::Lane;
    use crate::mastodon::{RemotePollVoteOutcome, activitypub};
    use crate::remote::RemoteFetchError;

    #[test]
    fn quote_fetch_references_do_not_require_authorization() {
        let references = remote_note_quote_fetch_references(&json!({
            "id": "https://remote.example/statuses/9",
            "quote": "https://target.example/statuses/7"
        }))
        .expect("the quote target should be retained");

        assert_eq!(references.target_uri, "https://target.example/statuses/7");
        assert!(references.approval_uri.is_none());
        for tombstone in [
            json!({ "quote": { "type": "Tombstone" } }),
            json!({
                "quote": {
                    "id": "https://target.example/statuses/deleted",
                    "type": "Tombstone"
                }
            }),
        ] {
            assert!(
                remote_note_quote_fetch_references(&tombstone).is_none(),
                "quoted Tombstones must reconcile as removal without dereference"
            );
        }
    }

    #[test]
    fn quote_delivery_metadata_binds_the_original_request() {
        let request_uri = "https://remote.example/quote_requests/1";
        let request = json!({
            "id": request_uri,
            "type": "QuoteRequest",
            "actor": "https://local.example/users/alice",
            "object": "https://remote.example/statuses/2",
            "instrument": { "id": "https://local.example/statuses/3" }
        });
        let request_arguments = json!({
            "quote_delivery_kind": "request",
            "quote_request_uri": request_uri,
            "quote_id": 4,
            "quoting_status_id": 3,
            "quoted_status_id": 2
        });
        let identity = quote_delivery_identity(&request_arguments, &request)
            .expect("metadata should parse")
            .expect("QuoteRequest is a quote delivery");
        assert_eq!(identity.kind, QuoteDeliveryKind::Request);
        assert_eq!(identity.quote_id, Some(4));

        for (kind, activity_type, result) in [
            (
                "accept",
                "Accept",
                Some("https://local.example/authorizations/4"),
            ),
            ("reject", "Reject", None),
        ] {
            let mut body = json!({
                "type": activity_type,
                "actor": "https://local.example/users/bob",
                "object": request
            });
            if let Some(result) = result {
                body["result"] = json!(result);
            }
            let arguments = json!({
                "quote_delivery_kind": kind,
                "quote_request_uri": request_uri,
                "quote_id": 4,
                "quoting_status_id": 3,
                "quoted_status_id": 2
            });
            let identity = quote_delivery_identity(&arguments, &body)
                .expect("metadata should parse")
                .expect("decision is a quote delivery");
            assert_eq!(
                identity.kind,
                if kind == "accept" {
                    QuoteDeliveryKind::Accept
                } else {
                    QuoteDeliveryKind::Reject
                }
            );
        }

        assert!(quote_delivery_identity(&json!({}), &request).is_err());
        let mut contradictory = request_arguments;
        contradictory["quote_request_uri"] = json!("https://remote.example/quote_requests/other");
        assert!(quote_delivery_identity(&contradictory, &request).is_err());
    }

    #[test]
    fn quote_request_instrument_requires_exact_bindings() {
        let actor_uri = "https://remote.example/users/alice";
        let instrument_uri = "https://remote.example/statuses/9";
        let target_uri = "https://local.example/users/bob/statuses/7";
        let valid = json!({
            "id": instrument_uri,
            "type": "Note",
            "attributedTo": actor_uri,
            "content": "a quote",
            "quote": target_uri
        });
        assert_eq!(
            quote_request_instrument_target_uri(&valid, actor_uri, instrument_uri)
                .expect("the exact Note should be accepted"),
            target_uri
        );
        let mut question = valid.clone();
        question["type"] = json!("Question");
        question
            .as_object_mut()
            .expect("Question is an object")
            .remove("quote");
        question["quoteUri"] = json!({ "id": target_uri });
        assert_eq!(
            quote_request_instrument_target_uri(&question, actor_uri, instrument_uri)
                .expect("Question and quote aliases should be accepted"),
            target_uri
        );

        for invalid in [
            json!(instrument_uri),
            json!({
                "id": instrument_uri,
                "type": "Article",
                "attributedTo": actor_uri,
                "content": "a quote",
                "quote": target_uri
            }),
            json!({
                "id": "https://remote.example/statuses/other",
                "type": "Note",
                "attributedTo": actor_uri,
                "content": "a quote",
                "quote": target_uri
            }),
            json!({
                "id": instrument_uri,
                "type": "Note",
                "attributedTo": "https://remote.example/users/mallory",
                "content": "a quote",
                "quote": target_uri
            }),
            json!({
                "id": instrument_uri,
                "type": "Note",
                "attributedTo": actor_uri,
                "content": "not a quote"
            }),
            json!({
                "id": instrument_uri,
                "type": "Note",
                "attributedTo": actor_uri,
                "content": "a quote",
                "quote": { "type": "Tombstone" }
            }),
            json!({
                "id": instrument_uri,
                "type": "Note",
                "attributedTo": actor_uri,
                "content": "a quote",
                "quote": "file:///tmp/status"
            }),
        ] {
            assert_eq!(
                quote_request_instrument_target_uri(&invalid, actor_uri, instrument_uri)
                    .expect_err("invalid QuoteRequest instruments must fail permanently")
                    .disposition,
                FailureDisposition::Permanent
            );
        }
    }

    #[test]
    fn quote_request_fetch_failures_distinguish_retryable_and_permanent_errors() {
        for error in [
            RemoteFetchError::UnexpectedStatus(StatusCode::NOT_FOUND),
            RemoteFetchError::UnexpectedStatus(StatusCode::REQUEST_TIMEOUT),
            RemoteFetchError::UnexpectedStatus(StatusCode::TOO_MANY_REQUESTS),
            RemoteFetchError::UnexpectedStatus(StatusCode::SERVICE_UNAVAILABLE),
            RemoteFetchError::NoAddresses,
            RemoteFetchError::Dns,
            RemoteFetchError::Client,
            RemoteFetchError::Request,
            RemoteFetchError::BodyRead,
            RemoteFetchError::DomainBudgetExceeded,
        ] {
            assert_eq!(
                remote_quote_request_fetch_failure(&error).disposition,
                FailureDisposition::Retry,
                "{error:?} should be retried"
            );
        }
        for error in [
            RemoteFetchError::UnexpectedStatus(StatusCode::BAD_REQUEST),
            RemoteFetchError::UnexpectedStatus(StatusCode::UNAUTHORIZED),
            RemoteFetchError::UnexpectedStatus(StatusCode::FORBIDDEN),
            RemoteFetchError::UnexpectedStatus(StatusCode::GONE),
            RemoteFetchError::UnexpectedStatus(StatusCode::NOT_ACCEPTABLE),
            RemoteFetchError::BlockedAddress("127.0.0.1".parse().expect("loopback address")),
            RemoteFetchError::InvalidUrl,
            RemoteFetchError::Redirect,
            RemoteFetchError::TooManyRedirects,
            RemoteFetchError::MissingContentType,
            RemoteFetchError::UnsupportedContentType,
            RemoteFetchError::UnsupportedEncoding,
            RemoteFetchError::BodyTooLarge,
            RemoteFetchError::InvalidRepresentation,
            RemoteFetchError::IdentityMismatch,
            RemoteFetchError::OriginMismatch,
            RemoteFetchError::PolicyDenied,
            RemoteFetchError::Signing,
        ] {
            assert_eq!(
                remote_quote_request_fetch_failure(&error).disposition,
                FailureDisposition::Permanent,
                "{error:?} should fail permanently"
            );
        }
    }

    #[test]
    fn quote_reference_uses_uri_when_remote_status_has_no_web_url() {
        assert_eq!(
            quote_reference(None, Some("https://remote.example/objects/quote")),
            Some("https://remote.example/objects/quote".to_owned())
        );
        assert_eq!(
            quote_reference(Some("  "), Some("https://remote.example/objects/quote")),
            Some("https://remote.example/objects/quote".to_owned())
        );
        assert_eq!(
            quote_reference(
                Some("https://remote.example/@alice/1"),
                Some("https://remote.example/objects/quote")
            ),
            Some("https://remote.example/@alice/1".to_owned())
        );
    }

    #[tokio::test]
    async fn poll_reconciliation_wall_time_cancels_the_whole_operation() {
        struct CancellationGuard(std::sync::Arc<std::sync::atomic::AtomicBool>);

        impl Drop for CancellationGuard {
            fn drop(&mut self) {
                self.0.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        }

        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let guard_flag = cancelled.clone();
        let started = tokio::time::Instant::now();
        let result = within_poll_expiration_reconciliation_wall_time(
            std::time::Duration::from_millis(10),
            async move {
                let _guard = CancellationGuard(guard_flag);
                std::future::pending::<Result<(), super::HandlerFailure>>().await
            },
        )
        .await;
        let failure = result.expect_err("the wall-time bound must cancel pending work");
        assert_eq!(failure.disposition, FailureDisposition::Retry);
        assert_eq!(
            failure.message,
            "poll expiration reconciliation wall time exceeded"
        );
        assert!(cancelled.load(std::sync::atomic::Ordering::SeqCst));
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
    }

    #[test]
    fn fallback_terminal_prefix_advances_without_spending_candidate_budget() {
        let mut cursor = 0_i64;
        let mut candidates = 0_usize;
        for poll_id in 1_i64..=33 {
            assert_eq!(
                poll_expiration_scan_step(true, candidates),
                PollExpirationScanStep::AdvanceTerminal
            );
            cursor = poll_id;
        }
        assert_eq!(cursor, 33);
        assert_eq!(candidates, 0);

        for poll_id in 34_i64..=58 {
            assert_eq!(
                poll_expiration_scan_step(false, candidates),
                PollExpirationScanStep::Reconcile
            );
            candidates += 1;
            cursor = poll_id;
        }
        assert_eq!(cursor, 58);
        assert_eq!(candidates, 25);
        assert_eq!(
            poll_expiration_scan_step(false, candidates),
            PollExpirationScanStep::Stop
        );
        assert_eq!(
            cursor, 58,
            "the unprocessed 26th actionable row remains behind the continuation cursor"
        );
    }

    #[test]
    fn reconciliation_segments_have_distinct_mode_bound_singleton_keys() {
        let started = Utc::now();
        let first = poll_expiration_reconciliation_job(started, Some(42), 7, 0);
        let continuation = poll_expiration_reconciliation_job(started, Some(42), 7, 1);
        let raw = poll_expiration_raw_reconciliation_job(started, 42, 7, 1);
        assert_ne!(first.logical_key_value(), continuation.logical_key_value());
        assert_ne!(continuation.logical_key_value(), raw.logical_key_value());
        assert_eq!(first.arguments()["segment"], 0);
        assert_eq!(continuation.arguments()["segment"], 1);
        assert_eq!(raw.arguments()["segment"], 1);
        assert_eq!(raw.arguments()["scan_mode"], "raw");
        assert_eq!(
            raw.logical_key_value(),
            Some(
                format!(
                    "poll-expiration-reconcile:{}:raw:1:7",
                    started.timestamp_micros()
                )
                .as_str()
            )
        );
        assert_eq!(
            poll_expiration_reconciliation_key_from_arguments(raw.arguments())
                .expect("raw arguments determine their exact singleton key"),
            raw.logical_key_value().expect("raw job has a key")
        );
        assert_eq!(
            parse_poll_expiration_scan_mode(raw.arguments(), 1)
                .expect("raw continuation shape is valid"),
            PollExpirationScanMode::Raw
        );
        assert!(
            parse_poll_expiration_scan_mode(&json!({"scan_mode": "raw"}), 0).is_err(),
            "raw mode is continuation-only"
        );
        assert!(parse_poll_expiration_scan_mode(&json!({"scan_mode": "unknown"}), 1).is_err());
        assert!(parse_poll_expiration_scan_mode(&json!({"scan_mode": 1}), 1).is_err());
    }

    #[test]
    fn reconciliation_retries_switch_optimized_jobs_to_raw_without_mutating_explicit_mode() {
        assert_eq!(
            poll_expiration_execution_mode(PollExpirationScanMode::Optimized, 1),
            PollExpirationScanMode::Optimized
        );
        assert_eq!(
            poll_expiration_execution_mode(PollExpirationScanMode::Optimized, 2),
            PollExpirationScanMode::Raw
        );
        assert_eq!(
            poll_expiration_execution_mode(PollExpirationScanMode::Raw, 1),
            PollExpirationScanMode::Raw
        );
        assert_eq!(
            poll_expiration_execution_mode(PollExpirationScanMode::Raw, 7),
            PollExpirationScanMode::Raw
        );
    }

    #[test]
    fn progress_after_retry_fallback_keeps_successors_raw() {
        let started = Utc::now();
        let initial = poll_expiration_reconciliation_job(started, Some(100), 0, 0);
        let first_execution = poll_expiration_execution_mode(
            parse_poll_expiration_scan_mode(initial.arguments(), 0)
                .expect("the initial optimized job has valid arguments"),
            2,
        );
        let first_successor =
            poll_expiration_reconciliation_continuation(started, Some(100), 25, 0, first_execution)
                .expect("the first progress-making successor is valid");
        assert_eq!(first_successor.arguments()["scan_mode"], "raw");
        assert!(
            first_successor
                .logical_key_value()
                .is_some_and(|key| key.contains(":raw:1:25"))
        );

        let configured_second = parse_poll_expiration_scan_mode(first_successor.arguments(), 1)
            .expect("the first progress-making successor has valid raw arguments");
        let second_execution = poll_expiration_execution_mode(configured_second, 1);
        let second_successor = poll_expiration_reconciliation_continuation(
            started,
            Some(100),
            50,
            1,
            second_execution,
        )
        .expect("the second progress-making successor is valid");
        assert_eq!(configured_second, PollExpirationScanMode::Raw);
        assert_eq!(second_successor.arguments()["scan_mode"], "raw");
        assert!(
            second_successor
                .logical_key_value()
                .is_some_and(|key| key.contains(":raw:2:50"))
        );
    }

    #[test]
    fn rejected_poll_votes_never_fall_back_to_ordinary_notes() {
        assert!(!remote_poll_vote_allows_note_fallback(
            RemotePollVoteOutcome::Consumed
        ));
        assert!(remote_poll_vote_allows_note_fallback(
            RemotePollVoteOutcome::NotPollVote
        ));
    }

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

        for activity_id in [
            activity_id,
            activitypub::update_activity_id(actor_uri, updated_at),
        ] {
            assert!(!update_delivery_is_current(
                Some(&activity_id),
                actor_uri,
                updated_at,
                Some(current_updated_at_micros - 1),
            ));
            assert!(update_delivery_is_current(
                Some(&activity_id),
                actor_uri,
                updated_at,
                Some(current_updated_at_micros),
            ));
        }
        let stale_id =
            activitypub::update_activity_id(actor_uri, updated_at - Duration::microseconds(1));
        assert!(!update_delivery_is_current(
            Some(&stale_id),
            actor_uri,
            updated_at,
            Some(current_updated_at_micros),
        ));
    }

    #[test]
    fn status_update_distribution_fences_both_versions_and_only_coalesces_poll_jobs() {
        let status_version = DateTime::<Utc>::from_timestamp(1_700_000_000, 0)
            .expect("valid timestamp")
            .naive_utc();
        let old_poll = status_version + Duration::microseconds(1);
        let current_poll = old_poll + Duration::microseconds(1);

        assert_eq!(
            status_update_versions(
                status_version,
                Some(current_poll),
                Some(status_version),
                Some(old_poll),
                StatusUpdateKind::Status,
                true,
            ),
            None,
            "a delayed status edit must not serialize a newer poll"
        );
        assert_eq!(
            status_update_versions(
                status_version,
                Some(current_poll),
                Some(status_version),
                Some(old_poll),
                StatusUpdateKind::Poll,
                true,
            ),
            Some((status_version, Some(current_poll), current_poll)),
            "a poll job may coalesce only while its status snapshot remains current"
        );
        assert_eq!(
            status_update_versions(
                status_version + Duration::microseconds(1),
                Some(current_poll),
                Some(status_version),
                Some(old_poll),
                StatusUpdateKind::Poll,
                true,
            ),
            None,
            "a poll job must repair rather than coalesce across status edits"
        );
        assert_eq!(
            status_update_version_decision(
                status_version + Duration::microseconds(2),
                Some(current_poll),
                Some(status_version),
                Some(current_poll),
                StatusUpdateKind::Poll,
                true,
            ),
            StatusUpdateVersionDecision::Repair,
            "a poll update stale only by a status edit must schedule combined repair"
        );
        let repaired_status = status_version + Duration::microseconds(3);
        assert_eq!(
            status_update_version_decision(
                repaired_status,
                Some(current_poll),
                Some(repaired_status),
                Some(current_poll),
                StatusUpdateKind::PollRepair,
                true,
            ),
            StatusUpdateVersionDecision::Deliver(
                repaired_status,
                Some(current_poll),
                repaired_status,
            ),
            "an exact combined repair uses the newest component and does not loop"
        );
        assert_eq!(
            status_update_version_decision(
                repaired_status,
                Some(current_poll + Duration::microseconds(1)),
                Some(repaired_status),
                Some(current_poll),
                StatusUpdateKind::PollRepair,
                true,
            ),
            StatusUpdateVersionDecision::Repair,
            "a superseded repair schedules one repair for the new pair"
        );
        assert!(StatusUpdateKind::PollRepair.has_poll_reach());
    }

    #[test]
    fn only_uri_only_quote_accepts_may_fall_back_to_follow_decisions() {
        assert!(quote_decision_allows_follow_fallback(
            true, None, None, None
        ));
        assert!(!quote_decision_allows_follow_fallback(
            false, None, None, None
        ));
        assert!(!quote_decision_allows_follow_fallback(
            true,
            Some("https://remote.example/users/alice"),
            None,
            None,
        ));
        assert!(!quote_decision_allows_follow_fallback(
            true,
            None,
            Some("https://local.example/statuses/1"),
            None,
        ));
    }

    #[test]
    fn interaction_policy_updates_use_the_status_row_update_version() {
        let policy_version = DateTime::<Utc>::from_timestamp(1_700_000_100, 123_000_000)
            .expect("valid timestamp")
            .naive_utc();
        let micros = policy_version.and_utc().timestamp_micros();
        assert_eq!(
            StatusUpdateKind::parse("interaction_policy"),
            Some(StatusUpdateKind::InteractionPolicy)
        );
        assert_eq!(
            status_update_versions(
                policy_version,
                None,
                Some(policy_version),
                None,
                StatusUpdateKind::InteractionPolicy,
                true,
            ),
            Some((policy_version, None, policy_version))
        );
        let object_uri = "https://local.example/users/alice/statuses/7";
        let activity_id = activitypub::update_activity_id(object_uri, policy_version);
        assert!(status_update_delivery_is_current(
            StatusUpdateKind::InteractionPolicy,
            Some(&activity_id),
            object_uri,
            policy_version,
            None,
            Some(micros),
            None,
            Some(micros),
        ));
    }

    #[test]
    fn quote_updates_preserve_edit_and_poll_snapshots_with_an_independent_version() {
        let status_version = DateTime::<Utc>::from_timestamp(1_700_000_000, 123_000_000)
            .expect("valid timestamp")
            .naive_utc();
        let poll_version = status_version + Duration::microseconds(1);
        let quote_version = poll_version + Duration::microseconds(1);
        let object_uri = "https://local.example/users/alice/statuses/7";
        let activity_id = activitypub::update_activity_id(object_uri, quote_version);
        let status_micros = status_version.and_utc().timestamp_micros();
        let poll_micros = poll_version.and_utc().timestamp_micros();
        let quote_micros = quote_version.and_utc().timestamp_micros();
        let revoked_quote_version = quote_version + Duration::microseconds(1);
        let revoked_quote_micros = revoked_quote_version.and_utc().timestamp_micros();

        assert!(quote_revision_is_current(
            Some(quote_version),
            Some(quote_micros),
        ));
        assert!(!quote_revision_is_current(
            Some(revoked_quote_version),
            Some(quote_micros),
        ));
        assert!(quote_revision_is_current(
            Some(revoked_quote_version),
            Some(revoked_quote_micros),
        ));

        assert_eq!(
            StatusUpdateKind::parse("quote"),
            Some(StatusUpdateKind::Quote)
        );
        assert_eq!(StatusUpdateKind::Quote.as_str(), "quote");
        assert!(!StatusUpdateKind::Quote.is_repair());
        assert!(!StatusUpdateKind::Quote.has_poll_reach());
        assert_eq!(
            quote_update_versions(
                status_version,
                Some(poll_version),
                Some(quote_version),
                Some(status_version),
                Some(poll_version),
                Some(quote_micros),
                Some(quote_version),
            ),
            Some((status_version, Some(poll_version), quote_version)),
        );
        assert_eq!(
            quote_update_versions(
                status_version,
                Some(poll_version),
                Some(quote_version),
                Some(status_version + Duration::microseconds(1)),
                Some(poll_version),
                Some(quote_micros),
                Some(quote_version),
            ),
            None,
            "a quote transition must not serialize across a status edit",
        );
        assert_eq!(
            complete_status_update_delivery_kind(
                Some("quote"),
                true,
                Some(status_micros),
                Some(poll_micros),
                Some(quote_micros),
                Some(quote_micros),
            ),
            Some(StatusUpdateKind::Quote),
        );
        assert!(status_update_delivery_is_current(
            StatusUpdateKind::Quote,
            Some(&activity_id),
            object_uri,
            status_version,
            Some(poll_version),
            Some(status_micros),
            Some(poll_micros),
            Some(quote_micros),
        ));
        assert!(!status_update_delivery_is_current(
            StatusUpdateKind::Quote,
            Some(&activity_id),
            object_uri,
            status_version + Duration::microseconds(1),
            Some(poll_version),
            Some(status_micros),
            Some(poll_micros),
            Some(quote_micros),
        ));
    }

    #[test]
    fn status_update_delivery_fences_status_and_poll_versions_independently() {
        let status_version = DateTime::<Utc>::from_timestamp(1_700_000_000, 123_000_000)
            .expect("valid timestamp")
            .naive_utc();
        let poll_version = status_version + Duration::microseconds(10);
        let next_poll_version = poll_version + Duration::microseconds(1);
        let object_uri = "https://local.example/users/alice/statuses/7";
        let activity_id = activitypub::update_activity_id(object_uri, status_version);
        let status_micros = status_version.and_utc().timestamp_micros();
        let poll_micros = poll_version.and_utc().timestamp_micros();

        assert!(status_update_delivery_is_current(
            StatusUpdateKind::Status,
            Some(&activity_id),
            object_uri,
            status_version,
            Some(poll_version),
            Some(status_micros),
            Some(poll_micros),
            Some(status_micros),
        ));
        let status_repair_activity_id =
            status_snapshot_repair_activity_id(object_uri, status_version, None);
        assert_ne!(status_repair_activity_id, activity_id);
        assert!(status_update_delivery_is_current(
            StatusUpdateKind::StatusRepair,
            Some(&status_repair_activity_id),
            object_uri,
            status_version,
            None,
            Some(status_micros),
            None,
            Some(status_micros),
        ));
        let poll_activity_id = activitypub::update_activity_id(object_uri, poll_version);
        assert!(status_update_delivery_is_current(
            StatusUpdateKind::Poll,
            Some(&poll_activity_id),
            object_uri,
            status_version,
            Some(poll_version),
            Some(status_micros),
            Some(poll_micros),
            Some(poll_micros),
        ));
        let repair_status_version = poll_version + Duration::microseconds(10);
        let repair_status_micros = repair_status_version.and_utc().timestamp_micros();
        let repair_activity_id = status_snapshot_repair_activity_id(
            object_uri,
            repair_status_version,
            Some(poll_version),
        );
        assert_ne!(
            repair_activity_id,
            status_snapshot_repair_activity_id(
                object_uri,
                repair_status_version,
                Some(next_poll_version),
            ),
            "repair identity must bind both snapshots even when their maximum is unchanged"
        );
        assert!(status_update_delivery_is_current(
            StatusUpdateKind::PollRepair,
            Some(&repair_activity_id),
            object_uri,
            repair_status_version,
            Some(poll_version),
            Some(repair_status_micros),
            Some(poll_micros),
            Some(repair_status_micros),
        ));
        assert_eq!(
            inferred_current_repair_delivery_kind(
                Some(&repair_activity_id),
                object_uri,
                repair_status_version,
                Some(poll_version),
                Some(repair_status_micros),
            ),
            Some(StatusUpdateKind::PollRepair),
            "an exact pair-bound repair remains deliverable if its metadata is stripped"
        );
        assert!(!status_update_delivery_is_current(
            StatusUpdateKind::Status,
            Some(&activity_id),
            object_uri,
            status_version,
            Some(next_poll_version),
            Some(status_micros),
            Some(poll_micros),
            Some(status_micros),
        ));
        assert!(!status_update_delivery_is_current(
            StatusUpdateKind::Status,
            Some(&activity_id),
            object_uri,
            status_version + Duration::microseconds(1),
            Some(poll_version),
            Some(status_micros),
            Some(poll_micros),
            Some(status_micros),
        ));
    }

    #[test]
    fn legacy_or_partial_status_update_delivery_metadata_requires_current_repair() {
        let micros = Some(1_700_000_000_000_000);
        assert_eq!(
            complete_status_update_delivery_kind(
                Some("poll_repair"),
                true,
                micros,
                micros,
                micros,
                micros,
            ),
            Some(StatusUpdateKind::PollRepair)
        );
        for (kind, edited, poll, version) in [
            (None, micros, micros, micros),
            (Some("poll"), None, micros, micros),
            (Some("poll"), micros, None, micros),
            (Some("poll"), micros, micros, None),
        ] {
            assert_eq!(
                complete_status_update_delivery_kind(kind, true, edited, poll, version, micros),
                None,
                "incomplete legacy metadata must schedule a current combined repair"
            );
        }
        assert_eq!(
            complete_status_update_delivery_kind(
                Some("status"),
                false,
                micros,
                None,
                micros,
                micros,
            ),
            Some(StatusUpdateKind::Status)
        );
        assert_eq!(
            complete_status_update_delivery_kind(
                Some("status"),
                false,
                micros,
                micros,
                micros,
                micros,
            ),
            None,
            "a contradictory poll snapshot must not make a legacy body current"
        );
        let later = micros.map(|value| value + 1);
        assert_eq!(
            complete_status_update_delivery_kind(Some("status"), true, micros, later, later, later,),
            None,
            "kind, published time, and selected version must agree"
        );
    }

    #[test]
    fn remote_create_wrapper_keeps_activity_and_note_uris_distinct() {
        let note_uri = "https://remote.example/users/alice/statuses/1";
        let create_uri = "https://remote.example/activities/1";
        let document = json!({
            "id": create_uri,
            "type": ["Create"],
            "actor": "https://remote.example/users/alice",
            "object": {
                "id": note_uri,
                "type": ["Question"],
                "attributedTo": "https://remote.example/users/alice",
                "content": "hello",
                "oneOf": [{"type": "Note", "name": "Tea"}]
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
            "type": ["Question"],
            "attributedTo": actor_uri,
            "content": "hello",
            "oneOf": [{"type": "Note", "name": "Tea"}]
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

#[cfg(test)]
mod activity_tests;
