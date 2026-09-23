mod push;
use push::*;
mod delivery;
use delivery::*;
mod pull;
use pull::*;
mod ingress;
use ingress::*;
mod poll_expiration;
pub use poll_expiration::*;
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
