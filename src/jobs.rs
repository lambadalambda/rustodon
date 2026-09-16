use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;
#[cfg(feature = "test-support")]
use std::sync::Arc;
#[cfg(feature = "test-support")]
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{DateTime, Duration, Utc};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{PgPool, Postgres, Row, Transaction};
use std::time::Duration as StdDuration;

use crate::streaming::{
    STREAM_EVENT_KIND, STREAM_HISTORY_MAX_AGE_HOURS, STREAM_HISTORY_MAX_EVENTS,
    STREAM_REPLAY_TRANSITION_EVENTS, STREAM_REPLAY_UPDATE_EVENTS, StreamEvent, StreamName,
    Subscription, TimelineRouteSnapshot,
};

const DEFAULT_MAX_ATTEMPTS: i32 = 25;
const MAX_ERROR_BYTES: usize = 4 * 1024;
pub const ACTIVITYPUB_INBOX_JOB_KIND: &str = "rustodon.activitypub.process_inbox";
pub const ACTIVITYPUB_INBOX_ORDERING_KIND: &str = "rustodon.activitypub.inbox";
pub const ACTIVITYPUB_DELIVERY_ORDERING_KIND: &str = "rustodon.activitypub.delivery";
pub const ACTIVITYPUB_INBOX_IDEMPOTENCY_SCOPE: &str = "rustodon.activitypub.inbox";
pub const ACTIVITYPUB_STATUS_DISTRIBUTION_JOB_KIND: &str = "rustodon.activitypub.distribute_status";
pub const ACTIVITYPUB_ACCOUNT_UPDATE_JOB_KIND: &str = "rustodon.activitypub.update_account";
pub const ACTIVITYPUB_ACCOUNT_DELETE_JOB_KIND: &str = "rustodon.activitypub.delete_account";
pub const MASTODON_ACCOUNT_PURGE_JOB_KIND: &str = "rustodon.mastodon.purge_account";
pub const MASTODON_DOMAIN_BLOCK_JOB_KIND: &str = "rustodon.mastodon.domain_block";
pub const ACTIVITYPUB_DELIVERY_JOB_KIND: &str = "rustodon.activitypub.deliver";
pub const ACTIVITYPUB_THREAD_RESOLVE_JOB_KIND: &str = "rustodon.activitypub.resolve_thread";
pub const ACTIVITYPUB_ANNOUNCE_RESOLVE_JOB_KIND: &str = "rustodon.activitypub.resolve_announce";
pub const ACTIVITYPUB_NOTE_RESOLVE_JOB_KIND: &str = "rustodon.activitypub.resolve_note";
pub const ACTIVITYPUB_PROFILE_MEDIA_FETCH_JOB_KIND: &str =
    "rustodon.activitypub.fetch_profile_media";
pub const ACTIVITYPUB_PROFILE_MEDIA_CLEANUP_JOB_KIND: &str =
    "rustodon.activitypub.cleanup_profile_media";
pub const ACTIVITYPUB_MEDIA_FETCH_JOB_KIND: &str = "rustodon.activitypub.fetch_media";
pub const ACTIVITYPUB_EMOJI_FETCH_JOB_KIND: &str = "rustodon.activitypub.fetch_emoji";
pub const ACTIVITYPUB_EMOJI_CLEANUP_JOB_KIND: &str = "rustodon.activitypub.cleanup_emoji";
pub const NOTIFICATION_CREATE_JOB_KIND: &str = "rustodon.mastodon.notify_activity";
pub const NOTIFICATION_UNFILTER_JOB_KIND: &str = "rustodon.mastodon.unfilter_notifications";
pub const NOTIFICATION_CLEANUP_JOB_KIND: &str = "rustodon.mastodon.cleanup_filtered_notifications";
pub const ACCOUNT_DELETION_DELAY_DAYS: i64 = 30;
pub const MASTODON_DOMAIN_PURGE_JOB_KIND: &str = "rustodon.mastodon.purge_domain";
pub const LOCAL_MEDIA_CLEANUP_JOB_KIND: &str = "rustodon.mastodon.cleanup_local_media";
const STREAM_EVENT_ORDERING_LOCK_KEY: &str = "rustodon.mastodon.stream_event.commit_order";
const STREAM_EVENT_STAGING_KIND: &str = "rustodon.mastodon.stream_event.staged";

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Lane {
    Ingress,
    Core,
    Push,
    Pull,
    Mail,
    Maintenance,
}

impl Lane {
    pub const ALL: [Self; 6] = [
        Self::Ingress,
        Self::Core,
        Self::Push,
        Self::Pull,
        Self::Mail,
        Self::Maintenance,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ingress => "ingress",
            Self::Core => "core",
            Self::Push => "push",
            Self::Pull => "pull",
            Self::Mail => "mail",
            Self::Maintenance => "maintenance",
        }
    }
}

impl fmt::Display for Lane {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for Lane {
    type Err = JobError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "ingress" => Ok(Self::Ingress),
            "core" => Ok(Self::Core),
            "push" => Ok(Self::Push),
            "pull" => Ok(Self::Pull),
            "mail" => Ok(Self::Mail),
            "maintenance" => Ok(Self::Maintenance),
            _ => Err(JobError::InvalidData("unknown durable-job lane")),
        }
    }
}

#[derive(Clone, Debug)]
pub struct JobSpec {
    lane: Lane,
    kind: String,
    arguments: Value,
    logical_key: Option<String>,
    run_at: DateTime<Utc>,
    max_attempts: i32,
}

impl JobSpec {
    #[must_use]
    pub fn new(lane: Lane, kind: impl Into<String>, arguments: Value) -> Self {
        Self {
            lane,
            kind: kind.into(),
            arguments,
            logical_key: None,
            run_at: Utc::now(),
            max_attempts: DEFAULT_MAX_ATTEMPTS,
        }
    }

    #[must_use]
    pub fn logical_key(mut self, logical_key: impl Into<String>) -> Self {
        self.logical_key = Some(logical_key.into());
        self
    }

    #[must_use]
    pub const fn run_at(mut self, run_at: DateTime<Utc>) -> Self {
        self.run_at = run_at;
        self
    }

    #[must_use]
    pub const fn run_at_value(&self) -> DateTime<Utc> {
        self.run_at
    }

    #[must_use]
    pub const fn max_attempts(mut self, max_attempts: i32) -> Self {
        self.max_attempts = max_attempts;
        self
    }

    #[must_use]
    pub const fn lane(&self) -> Lane {
        self.lane
    }

    #[must_use]
    pub fn kind(&self) -> &str {
        &self.kind
    }

    #[must_use]
    pub const fn arguments(&self) -> &Value {
        &self.arguments
    }

    #[must_use]
    pub fn logical_key_value(&self) -> Option<&str> {
        self.logical_key.as_deref()
    }

    fn validate(&self) -> Result<(), JobError> {
        if !(1..=128).contains(&self.kind.len()) {
            return Err(JobError::InvalidInput("job kind must contain 1-128 bytes"));
        }
        if self
            .logical_key
            .as_ref()
            .is_some_and(|key| !(1..=1024).contains(&key.len()))
        {
            return Err(JobError::InvalidInput(
                "logical key must contain 1-1024 bytes",
            ));
        }
        if self.max_attempts <= 0 {
            return Err(JobError::InvalidInput("max attempts must be positive"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct ClaimedJob {
    pub id: i64,
    pub lane: Lane,
    pub kind: String,
    pub arguments: Value,
    pub logical_key: Option<String>,
    pub run_at: DateTime<Utc>,
    pub attempt: i32,
    pub max_attempts: i32,
    pub generation: i64,
    pub lease_owner: String,
    pub lease_expires_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RetryResult {
    Scheduled,
    Dead,
    Lost,
}

#[derive(Clone, Debug)]
pub struct DeadLetter {
    pub id: i64,
    pub lane: Lane,
    pub kind: String,
    pub attempts: i32,
    pub max_attempts: i32,
    pub last_error: Option<String>,
    pub dead_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct WorkerHeartbeat {
    process_id: String,
    role: &'static str,
    lanes: Vec<Lane>,
    info: Value,
}

impl WorkerHeartbeat {
    #[must_use]
    pub fn worker(
        process_id: impl Into<String>,
        lanes: impl IntoIterator<Item = Lane>,
        info: Value,
    ) -> Self {
        Self {
            process_id: process_id.into(),
            role: "worker",
            lanes: lanes.into_iter().collect(),
            info,
        }
    }

    #[must_use]
    pub fn scheduler(process_id: impl Into<String>, info: Value) -> Self {
        Self {
            process_id: process_id.into(),
            role: "scheduler",
            lanes: Vec::new(),
            info,
        }
    }

    fn validate(&self) -> Result<(), JobError> {
        if !(1..=255).contains(&self.process_id.len()) {
            return Err(JobError::InvalidInput(
                "process ID must contain 1-255 bytes",
            ));
        }
        if self.role == "worker" && self.lanes.is_empty() {
            return Err(JobError::InvalidInput(
                "worker heartbeat must advertise a lane",
            ));
        }
        if !self.info.is_object() {
            return Err(JobError::InvalidInput(
                "heartbeat info must be a JSON object",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct Readiness {
    pub missing_lanes: BTreeSet<Lane>,
    pub scheduler_alive: bool,
    pub dead_letters: i64,
    pub queued_jobs: i64,
    pub oldest_queued_at: Option<DateTime<Utc>>,
}

impl Readiness {
    #[must_use]
    pub fn ready(&self) -> bool {
        self.missing_lanes.is_empty() && self.scheduler_alive
    }
}

#[derive(Debug)]
pub enum JobError {
    Sqlx(sqlx::Error),
    Conflict(&'static str),
    InvalidInput(&'static str),
    InvalidData(&'static str),
}

impl fmt::Display for JobError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlx(_) => formatter.write_str("PostgreSQL rejected a durable-job operation"),
            Self::Conflict(message) | Self::InvalidInput(message) | Self::InvalidData(message) => {
                formatter.write_str(message)
            }
        }
    }
}

impl std::error::Error for JobError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Sqlx(error) => Some(error),
            Self::Conflict(_) | Self::InvalidInput(_) | Self::InvalidData(_) => None,
        }
    }
}

impl From<sqlx::Error> for JobError {
    fn from(error: sqlx::Error) -> Self {
        Self::Sqlx(error)
    }
}

#[derive(Clone)]
pub struct Queue {
    pool: PgPool,
    #[cfg(feature = "test-support")]
    fail_complete_once: Arc<AtomicBool>,
}

impl Queue {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            #[cfg(feature = "test-support")]
            fail_complete_once: Arc::new(AtomicBool::new(false)),
        }
    }

    #[cfg(feature = "test-support")]
    #[must_use]
    pub fn with_complete_fault(self) -> Self {
        self.fail_complete_once.store(true, Ordering::SeqCst);
        self
    }

    #[must_use]
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Enqueues a job in its own transaction.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid job metadata or a rejected database operation.
    pub async fn enqueue(&self, spec: &JobSpec) -> Result<i64, JobError> {
        let mut transaction = self.pool.begin().await?;
        let id = enqueue_in(&mut transaction, spec).await?;
        transaction.commit().await?;
        Ok(id)
    }

    /// Enqueues a job while serializing jobs sharing an ordering key.
    ///
    /// The marker and durable job are committed together, so a request cannot advertise an
    /// ordering position that was not durably accepted.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid job metadata or a rejected database operation.
    pub async fn enqueue_ordered(
        &self,
        spec: &JobSpec,
        ordering_key: &[u8; 32],
    ) -> Result<i64, JobError> {
        let mut transaction = self.pool.begin().await?;
        let id = enqueue_ordered_in(&mut transaction, spec, ordering_key).await?;
        transaction.commit().await?;
        Ok(id)
    }

    /// Enqueues one logical `ActivityPub` activity, retaining its deduplication marker after the
    /// durable job is acknowledged by a worker.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid job metadata or a rejected database operation.
    pub async fn enqueue_ordered_once(
        &self,
        spec: &JobSpec,
        ordering_key: &[u8; 32],
        fingerprint: &[u8; 32],
    ) -> Result<bool, JobError> {
        spec.validate()?;
        let logical_key = spec.logical_key_value().ok_or(JobError::InvalidInput(
            "deduplicated jobs require a logical key",
        ))?;
        let mut transaction = self.pool.begin().await?;
        let inserted = sqlx::query_scalar::<_, String>(
            "INSERT INTO rustodon.idempotency_keys \
                (scope, key, fingerprint, result, expires_at) \
             VALUES ($1, $2, $3, $4, clock_timestamp() + interval '30 days') \
             ON CONFLICT (scope, key) DO UPDATE SET \
                fingerprint = EXCLUDED.fingerprint, result = EXCLUDED.result, \
                created_at = clock_timestamp(), expires_at = EXCLUDED.expires_at \
             WHERE rustodon.idempotency_keys.expires_at <= clock_timestamp() \
             RETURNING key",
        )
        .bind(ACTIVITYPUB_INBOX_IDEMPOTENCY_SCOPE)
        .bind(logical_key)
        .bind(fingerprint.as_slice())
        .bind(json!({}))
        .fetch_optional(&mut *transaction)
        .await?;
        if inserted.is_none() {
            let existing_fingerprint = sqlx::query_scalar::<_, Vec<u8>>(
                "SELECT fingerprint FROM rustodon.idempotency_keys \
                 WHERE scope = $1 AND key = $2",
            )
            .bind(ACTIVITYPUB_INBOX_IDEMPOTENCY_SCOPE)
            .bind(logical_key)
            .fetch_optional(&mut *transaction)
            .await?;
            if existing_fingerprint.as_deref() != Some(fingerprint.as_slice()) {
                return Err(JobError::Conflict(
                    "ActivityPub activity body conflicts with an existing logical activity",
                ));
            }
            transaction.commit().await?;
            return Ok(false);
        }
        let id = enqueue_ordered_in(&mut transaction, spec, ordering_key).await?;
        sqlx::query(
            "UPDATE rustodon.idempotency_keys SET result = jsonb_build_object('job_id', $3) \
             WHERE scope = $1 AND key = $2",
        )
        .bind(ACTIVITYPUB_INBOX_IDEMPOTENCY_SCOPE)
        .bind(logical_key)
        .bind(id)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(true)
    }

    /// Claims one due job while fencing stale workers with a lease generation.
    ///
    /// # Errors
    ///
    /// Returns an error when metadata is invalid or `PostgreSQL` rejects the claim.
    pub async fn claim(
        &self,
        lease_owner: &str,
        lanes: &[Lane],
        lease_duration: Duration,
    ) -> Result<Option<ClaimedJob>, JobError> {
        validate_lease(lease_owner, lease_duration)?;
        if lanes.is_empty() {
            return Err(JobError::InvalidInput("claim requires at least one lane"));
        }
        let lanes = lanes.iter().map(|lane| lane.as_str()).collect::<Vec<_>>();
        let lease_milliseconds = lease_duration.num_milliseconds();
        let mut transaction = self.pool.begin().await?;
        // Ordered enqueue allocates IDs under the stream marker lock. Fence every earlier
        // live member, not just the immediate predecessor: cancellation may delete that link,
        // and retries may move its run_at beyond its successors.
        let row = sqlx::query(
            "WITH candidate AS ( \
               SELECT job.id FROM rustodon.durable_jobs job \
               WHERE job.dead_at IS NULL AND job.run_at <= clock_timestamp() \
                 AND (job.lease_expires_at IS NULL OR job.lease_expires_at <= clock_timestamp()) \
                 AND job.lane = ANY($1) \
                 AND NOT EXISTS ( \
                   SELECT 1 FROM rustodon.durable_jobs predecessor \
                   WHERE predecessor.id = CASE \
                     WHEN job.arguments ->> '_rustodon_ordering_predecessor' ~ '^[0-9]+$' \
                     THEN (job.arguments ->> '_rustodon_ordering_predecessor')::bigint \
                    ELSE NULL \
                   END \
                     AND predecessor.dead_at IS NULL \
                     AND (predecessor.arguments ->> '_rustodon_ordering_key' IS NULL \
                          OR predecessor.arguments ->> '_rustodon_ordering_key' = \
                             job.arguments ->> '_rustodon_ordering_key')) \
                 AND NOT EXISTS ( \
                   SELECT 1 FROM rustodon.durable_jobs earlier \
                   WHERE earlier.kind = job.kind AND earlier.id < job.id \
                     AND earlier.dead_at IS NULL \
                     AND earlier.arguments ->> '_rustodon_ordering_key' = \
                         job.arguments ->> '_rustodon_ordering_key') \
               ORDER BY job.run_at, job.id FOR UPDATE OF job SKIP LOCKED LIMIT 1) \
             UPDATE rustodon.durable_jobs job \
              SET attempts = CASE WHEN attempts < max_attempts THEN attempts + 1 ELSE attempts END, \
                 lease_generation = lease_generation + 1, \
                 lease_owner = $2, \
                 lease_expires_at = clock_timestamp() \
                   + make_interval(secs => $3::double precision / 1000), \
                 updated_at = clock_timestamp() \
             FROM candidate WHERE job.id = candidate.id \
             RETURNING job.id, job.lane, job.kind, job.arguments, job.logical_key, job.run_at, \
                       job.attempts, job.max_attempts, job.lease_generation, job.lease_owner, \
                       job.lease_expires_at",
        )
        .bind(&lanes)
        .bind(lease_owner)
        .bind(lease_milliseconds)
        .fetch_optional(&mut *transaction)
        .await?;
        transaction.commit().await?;
        row.as_ref().map(claimed_job).transpose()
    }

    /// Extends a live lease only when its owner and generation still match.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid lease metadata or a rejected database operation.
    pub async fn renew(
        &self,
        id: i64,
        lease_owner: &str,
        generation: i64,
        lease_duration: Duration,
    ) -> Result<bool, JobError> {
        validate_lease(lease_owner, lease_duration)?;
        let result = sqlx::query(
            "UPDATE rustodon.durable_jobs \
             SET lease_expires_at = clock_timestamp() \
                   + make_interval(secs => $4::double precision / 1000), \
                 updated_at = clock_timestamp() \
             WHERE id = $1 AND lease_owner = $2 AND lease_generation = $3 \
               AND dead_at IS NULL AND lease_expires_at > clock_timestamp()",
        )
        .bind(id)
        .bind(lease_owner)
        .bind(generation)
        .bind(lease_duration.num_milliseconds())
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    /// Merges durable metadata into a live job while retaining the lease fence.
    ///
    /// # Errors
    ///
    /// Returns an error for a non-object patch or a rejected database update.
    pub async fn merge_job_arguments(
        &self,
        job: &ClaimedJob,
        patch: &Value,
    ) -> Result<bool, JobError> {
        if !patch.is_object() {
            return Err(JobError::InvalidInput(
                "durable-job argument patches must be JSON objects",
            ));
        }
        let result = sqlx::query(
            "UPDATE rustodon.durable_jobs \
                SET arguments = arguments || $4::jsonb, updated_at = clock_timestamp() \
              WHERE id = $1 AND lease_owner = $2 AND lease_generation = $3 \
                AND dead_at IS NULL AND lease_expires_at > clock_timestamp()",
        )
        .bind(job.id)
        .bind(&job.lease_owner)
        .bind(job.generation)
        .bind(patch)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    /// Initializes absent durable arguments while retaining the lease fence and existing values.
    ///
    /// # Errors
    ///
    /// Returns an error for a non-object patch or a rejected database update.
    pub async fn initialize_job_arguments(
        &self,
        job: &ClaimedJob,
        patch: &Value,
    ) -> Result<Option<Value>, JobError> {
        if !patch.is_object() {
            return Err(JobError::InvalidInput(
                "durable-job argument patches must be JSON objects",
            ));
        }
        sqlx::query_scalar(
            "UPDATE rustodon.durable_jobs \
                SET arguments = $4::jsonb || arguments, updated_at = clock_timestamp() \
              WHERE id = $1 AND lease_owner = $2 AND lease_generation = $3 \
                AND dead_at IS NULL AND lease_expires_at > clock_timestamp() \
              RETURNING arguments",
        )
        .bind(job.id)
        .bind(&job.lease_owner)
        .bind(job.generation)
        .bind(patch)
        .fetch_optional(&self.pool)
        .await
        .map_err(Into::into)
    }

    /// Deletes a completed job only while the caller still owns its lease.
    ///
    /// # Errors
    ///
    /// Returns an error when `PostgreSQL` rejects the acknowledgement.
    pub async fn complete(
        &self,
        id: i64,
        lease_owner: &str,
        generation: i64,
    ) -> Result<bool, JobError> {
        #[cfg(feature = "test-support")]
        if self.fail_complete_once.swap(false, Ordering::SeqCst) {
            return Err(JobError::InvalidData(
                "injected durable-job completion failure",
            ));
        }
        let result = sqlx::query(
            "DELETE FROM rustodon.durable_jobs \
              WHERE id = $1 AND lease_owner = $2 AND lease_generation = $3 AND dead_at IS NULL \
                AND lease_expires_at > clock_timestamp()",
        )
        .bind(id)
        .bind(lease_owner)
        .bind(generation)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    /// Releases a failed job for a delayed retry or moves an exhausted job to the dead-letter set.
    ///
    /// # Errors
    ///
    /// Returns an error when `PostgreSQL` rejects the fenced transition.
    pub async fn retry(
        &self,
        job: &ClaimedJob,
        run_at: DateTime<Utc>,
        error: &str,
    ) -> Result<RetryResult, JobError> {
        let error = bounded(error, MAX_ERROR_BYTES);
        let (dead_at, result) = if job.attempt >= job.max_attempts {
            (
                true,
                sqlx::query(
                    "UPDATE rustodon.durable_jobs \
                     SET lease_owner = NULL, lease_expires_at = NULL, last_error = $4, \
                         dead_at = clock_timestamp(), updated_at = clock_timestamp() \
                     WHERE id = $1 AND lease_owner = $2 AND lease_generation = $3 \
                        AND dead_at IS NULL AND lease_expires_at > clock_timestamp()",
                )
                .bind(job.id)
                .bind(&job.lease_owner)
                .bind(job.generation)
                .bind(error)
                .execute(&self.pool)
                .await?,
            )
        } else {
            (
                false,
                sqlx::query(
                    "UPDATE rustodon.durable_jobs \
                     SET lease_owner = NULL, lease_expires_at = NULL, last_error = $4, \
                         run_at = $5, updated_at = clock_timestamp() \
                     WHERE id = $1 AND lease_owner = $2 AND lease_generation = $3 \
                        AND dead_at IS NULL AND lease_expires_at > clock_timestamp()",
                )
                .bind(job.id)
                .bind(&job.lease_owner)
                .bind(job.generation)
                .bind(error)
                .bind(run_at)
                .execute(&self.pool)
                .await?,
            )
        };
        Ok(if result.rows_affected() == 0 {
            RetryResult::Lost
        } else if dead_at {
            RetryResult::Dead
        } else {
            RetryResult::Scheduled
        })
    }

    /// Moves a permanently failed job directly to the dead-letter set.
    ///
    /// # Errors
    ///
    /// Returns an error when `PostgreSQL` rejects the fenced transition.
    pub async fn dead_letter(&self, job: &ClaimedJob, error: &str) -> Result<bool, JobError> {
        let result = sqlx::query(
            "UPDATE rustodon.durable_jobs \
             SET lease_owner = NULL, lease_expires_at = NULL, last_error = $4, \
                 dead_at = clock_timestamp(), updated_at = clock_timestamp() \
             WHERE id = $1 AND lease_owner = $2 AND lease_generation = $3 \
                AND dead_at IS NULL AND lease_expires_at > clock_timestamp()",
        )
        .bind(job.id)
        .bind(&job.lease_owner)
        .bind(job.generation)
        .bind(bounded(error, MAX_ERROR_BYTES))
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    /// Cancels pending work with an exact kind and logical key.
    ///
    /// # Errors
    ///
    /// Leased jobs are fenced from cancellation because their handlers may already be running.
    /// Returns an error when `PostgreSQL` rejects the cancellation.
    pub async fn cancel(&self, kind: &str, logical_key: &str) -> Result<u64, JobError> {
        let mut transaction = self.pool.begin().await?;
        // Dispatch locks the same row before creating a job. Taking that lock first makes a
        // cancellation ordered after an in-progress dispatch remove the job it just created.
        sqlx::query(
            "SELECT id FROM rustodon.outbox_events \
             WHERE kind = $1 AND logical_key = $2 FOR UPDATE",
        )
        .bind(kind)
        .bind(logical_key)
        .fetch_optional(&mut *transaction)
        .await?;
        let jobs = sqlx::query(
            "DELETE FROM rustodon.durable_jobs \
             WHERE kind = $1 AND logical_key = $2 AND dead_at IS NULL \
               AND (lease_owner IS NULL OR lease_expires_at <= clock_timestamp())",
        )
        .bind(kind)
        .bind(logical_key)
        .execute(&mut *transaction)
        .await?
        .rows_affected();
        let events = sqlx::query(
            "DELETE FROM rustodon.outbox_events \
             WHERE kind = $1 AND logical_key = $2 AND dispatched_at IS NULL",
        )
        .bind(kind)
        .bind(logical_key)
        .execute(&mut *transaction)
        .await?
        .rows_affected();
        transaction.commit().await?;
        Ok(jobs.saturating_add(events))
    }

    /// Atomically converts pending outbox records to durable jobs.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed outbox data or a rejected database transaction.
    pub async fn dispatch_outbox(&self, limit: i64) -> Result<u64, JobError> {
        if limit <= 0 {
            return Err(JobError::InvalidInput("outbox limit must be positive"));
        }
        let mut transaction = self.pool.begin().await?;
        let rows = sqlx::query(
            "SELECT event.id, event.kind, event.logical_key, event.payload \
              FROM rustodon.outbox_events event WHERE event.dispatched_at IS NULL \
                AND event.kind <> 'rustodon.mastodon.stream_event' \
                AND NOT EXISTS ( \
                  SELECT 1 FROM rustodon.outbox_events previous \
                   WHERE event.kind = $1 AND previous.kind = event.kind \
                     AND previous.dispatched_at IS NULL AND previous.id < event.id \
                     AND previous.payload #>> '{arguments,source_account_id}' = \
                         event.payload #>> '{arguments,source_account_id}' \
                     AND previous.payload #>> '{arguments,inbox_url}' = \
                         event.payload #>> '{arguments,inbox_url}' \
                ) \
                AND (event.logical_key IS NULL OR NOT EXISTS ( \
                   SELECT 1 FROM rustodon.durable_jobs job \
                   WHERE job.kind = event.kind AND job.logical_key = event.logical_key \
                    AND job.dead_at IS NULL)) \
                ORDER BY event.id FOR UPDATE OF event SKIP LOCKED LIMIT $2",
        )
        .bind(ACTIVITYPUB_DELIVERY_JOB_KIND)
        .bind(limit)
        .fetch_all(&mut *transaction)
        .await?;
        let mut dispatched = 0_u64;
        for row in &rows {
            let id: i64 = row.try_get("id")?;
            let kind: String = row.try_get("kind")?;
            let logical_key: Option<String> = row.try_get("logical_key")?;
            let payload: Value = row.try_get("payload")?;
            let spec = outbox_spec(kind, logical_key, &payload)?;
            let inserted = if let Some(ordering_key) = activitypub_delivery_ordering_key(&spec) {
                enqueue_ordered_kind_in(
                    &mut transaction,
                    &spec,
                    &ordering_key,
                    ACTIVITYPUB_DELIVERY_ORDERING_KIND,
                )
                .await
                .map(|_| true)?
            } else {
                enqueue_outbox_in(&mut transaction, &spec).await?
            };
            if inserted {
                sqlx::query(
                    "UPDATE rustodon.outbox_events SET dispatched_at = clock_timestamp() \
                     WHERE id = $1 AND dispatched_at IS NULL",
                )
                .bind(id)
                .execute(&mut *transaction)
                .await?;
                dispatched = dispatched.saturating_add(1);
            }
        }
        transaction.commit().await?;
        Ok(dispatched)
    }

    /// Reports whether notification unfilter work is still queued for an account.
    ///
    /// # Errors
    ///
    /// Returns an error when `PostgreSQL` cannot read the operational queue.
    pub async fn notification_unfilter_pending(&self, account_id: i64) -> Result<bool, JobError> {
        Ok(sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS ( \
                SELECT 1 FROM rustodon.outbox_events event \
                 WHERE event.kind = $1 AND event.dispatched_at IS NULL \
                   AND event.payload #>> '{arguments,account_id}' = $2 \
                UNION ALL \
                SELECT 1 FROM rustodon.durable_jobs job \
                 WHERE job.kind = $1 AND job.dead_at IS NULL \
                   AND job.arguments ->> 'account_id' = $2 \
            )",
        )
        .bind(NOTIFICATION_UNFILTER_JOB_KIND)
        .bind(account_id.to_string())
        .fetch_one(&self.pool)
        .await?)
    }

    /// Returns the latest committed stream-event cursor.
    ///
    /// # Errors
    ///
    /// Returns an error when `PostgreSQL` cannot read the stream-event table.
    pub async fn stream_cursor(&self) -> Result<i64, JobError> {
        Ok(sqlx::query_scalar::<_, Option<i64>>(
            "SELECT max(id) FROM rustodon.outbox_events \
             WHERE kind = 'rustodon.mastodon.stream_event'",
        )
        .fetch_one(&self.pool)
        .await?
        .unwrap_or_default())
    }

    /// Prunes retained stream events to the configured age and count bounds and returns the cursor
    /// immediately before the oldest retained event. Retention covers bounded replay for new and
    /// reconnected timeline subscriptions.
    ///
    /// # Errors
    ///
    /// Returns an error when PostgreSQL cannot lock or prune the stream-event table.
    pub async fn prune_stream_history(&self) -> Result<i64, JobError> {
        self.prune_stream_history_with_limits(
            Duration::hours(STREAM_HISTORY_MAX_AGE_HOURS),
            STREAM_HISTORY_MAX_EVENTS,
        )
        .await
    }

    /// Prunes stream history with explicit bounds. This is public so isolated operational-schema
    /// tests can exercise retention with small deterministic limits.
    ///
    /// # Errors
    ///
    /// Returns an error for non-positive bounds or when PostgreSQL rejects the prune.
    pub async fn prune_stream_history_with_limits(
        &self,
        max_age: Duration,
        max_events: i64,
    ) -> Result<i64, JobError> {
        if max_age <= Duration::zero() || max_events <= 0 {
            return Err(JobError::InvalidInput(
                "stream history age and event limit must be positive",
            ));
        }
        let mut transaction = self.pool.begin().await?;
        sqlx::query(
            "DELETE FROM rustodon.outbox_events \
              WHERE kind = 'rustodon.mastodon.stream_event' \
                AND created_at < clock_timestamp() \
                  - make_interval(secs => $1::double precision / 1000)",
        )
        .bind(max_age.num_milliseconds())
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "DELETE FROM rustodon.outbox_events \
              WHERE kind = 'rustodon.mastodon.stream_event' AND id <= COALESCE(( \
                SELECT id FROM rustodon.outbox_events \
                 WHERE kind = 'rustodon.mastodon.stream_event' \
                 ORDER BY id DESC OFFSET $1 LIMIT 1), -1)",
        )
        .bind(max_events)
        .execute(&mut *transaction)
        .await?;
        let oldest = sqlx::query_scalar::<_, Option<i64>>(
            "SELECT min(id) FROM rustodon.outbox_events \
             WHERE kind = 'rustodon.mastodon.stream_event'",
        )
        .fetch_one(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(oldest.map_or(0, |id| id.saturating_sub(1)))
    }

    /// Reads immutable stream events after a cursor without consuming them.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid cursor parameters or when `PostgreSQL` rejects the read.
    pub async fn stream_events_after(
        &self,
        cursor: i64,
        limit: i64,
    ) -> Result<Vec<StreamEvent>, JobError> {
        if cursor < 0 || limit <= 0 {
            return Err(JobError::InvalidInput(
                "stream cursor must be non-negative and limit must be positive",
            ));
        }
        let rows = sqlx::query(
            "SELECT id, (payload ->> 'account_id')::bigint AS account_id, \
                    payload ->> 'event' AS event, \
                    (payload ->> 'object_id')::bigint AS object_id, \
                    payload -> 'before' AS before, payload -> 'after' AS after \
               FROM rustodon.outbox_events \
              WHERE kind = 'rustodon.mastodon.stream_event' AND id > $1 \
              ORDER BY id LIMIT $2",
        )
        .bind(cursor)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(stream_event_from_row).collect()
    }

    /// Returns a bounded retained suffix for a newly established timeline subscription.
    ///
    /// Lifecycle transitions are retained separately from create/update frames so a busy create
    /// stream cannot evict a recent delete. Results are merged back into durable event order.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid bounds or when PostgreSQL rejects the read.
    pub async fn stream_replay_events(&self, through: i64) -> Result<Vec<StreamEvent>, JobError> {
        self.stream_replay_events_with_limits(
            through,
            STREAM_REPLAY_TRANSITION_EVENTS,
            STREAM_REPLAY_UPDATE_EVENTS,
        )
        .await
    }

    /// Returns route-relevant retained events for one newly authorized timeline subscription.
    ///
    /// Structural route filtering happens before each historical event-class limit, preventing
    /// unrelated public, hashtag, or list traffic from evicting a recoverable event. Historical
    /// deletes and non-creating transitions have their own reserve so a burst of edits cannot
    /// displace deletes. Every event committed after `create_after` is returned without a
    /// per-class cap to make the subscribe handoff lossless. Historical durable events whose route
    /// transition would create a wire-level `update` are omitted; idempotent edits and actual
    /// deletes remain replayable for reconnect convergence.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid bounds or when PostgreSQL rejects the read.
    pub async fn stream_replay_events_for_subscription(
        &self,
        through: i64,
        create_after: i64,
        subscription: &Subscription,
        account_id: i64,
    ) -> Result<Vec<StreamEvent>, JobError> {
        if through < 0 || create_after < 0 || account_id <= 0 || !subscription.is_timeline() {
            return Err(JobError::InvalidInput(
                "timeline replay requires non-negative cursors, a positive account, and a timeline subscription",
            ));
        }
        let stream = subscription.stream();
        let parameter = subscription.parameter().unwrap_or("");
        let local = matches!(
            stream,
            StreamName::PublicLocal | StreamName::PublicLocalMedia | StreamName::HashtagLocal
        );
        let remote = matches!(
            stream,
            StreamName::PublicRemote | StreamName::PublicRemoteMedia
        );
        let media = matches!(
            stream,
            StreamName::PublicMedia | StreamName::PublicLocalMedia | StreamName::PublicRemoteMedia
        );
        let rows = sqlx::query(
            "WITH memberships AS ( \
               SELECT event.id, event.payload, \
                      COALESCE(bool_or(route.route_matches) \
                        FILTER (WHERE route.position = 'before'), false) AS before_matches, \
                      COALESCE(bool_or(route.route_matches) \
                        FILTER (WHERE route.position = 'after'), false) AS after_matches \
                 FROM rustodon.outbox_events event \
                 CROSS JOIN LATERAL ( \
                   SELECT candidate.position, candidate.snapshot IS NOT NULL AND ( \
                     ($3 = 'public' AND COALESCE((candidate.snapshot ->> 'public')::boolean, false) \
                       AND (NOT $4 OR COALESCE((candidate.snapshot ->> 'local')::boolean, false)) \
                       AND (NOT $5 OR NOT COALESCE((candidate.snapshot ->> 'local')::boolean, false)) \
                       AND (NOT $6 OR COALESCE((candidate.snapshot ->> 'had_media')::boolean, false))) \
                     OR ($3 = 'hashtag' \
                       AND COALESCE((candidate.snapshot ->> 'hashtag')::boolean, false) \
                       AND (NOT $4 OR COALESCE((candidate.snapshot ->> 'local')::boolean, false)) \
                       AND COALESCE(candidate.snapshot -> 'tags', '[]'::jsonb) ? $7) \
                     OR ($3 = 'list' AND COALESCE(candidate.snapshot -> 'lists', '[]'::jsonb) \
                       @> jsonb_build_array(jsonb_build_object( \
                         'account_id', $9::bigint, 'list_id', $8::bigint))) \
                   ) AS route_matches \
                     FROM (VALUES ('before', event.payload -> 'before'), \
                                  ('after', event.payload -> 'after')) \
                       candidate(position, snapshot) \
                 ) route \
                WHERE event.kind = 'rustodon.mastodon.stream_event' AND event.id <= $1 \
                  AND event.payload ->> 'account_id' = '0' \
                GROUP BY event.id, event.payload \
             ), candidates AS ( \
               SELECT id, payload, before_matches, after_matches FROM memberships \
                WHERE before_matches OR after_matches \
             ), selected AS ( \
               SELECT id FROM candidates WHERE id > $10 \
               UNION \
               SELECT id FROM (SELECT id FROM candidates \
                                WHERE id <= $10 AND payload ->> 'event' = 'delete' \
                                ORDER BY id DESC LIMIT $2) deletes \
               UNION \
               SELECT id FROM (SELECT id FROM candidates \
                                WHERE id <= $10 \
                                  AND payload ->> 'event' NOT IN ('update', 'delete') \
                                  AND (before_matches OR NOT after_matches) \
                                ORDER BY id DESC LIMIT $2) transitions \
             ) \
             SELECT event.id, (event.payload ->> 'account_id')::bigint AS account_id, \
                    event.payload ->> 'event' AS event, \
                    (event.payload ->> 'object_id')::bigint AS object_id, \
                    event.payload -> 'before' AS before, event.payload -> 'after' AS after \
               FROM rustodon.outbox_events event JOIN selected ON selected.id = event.id \
              ORDER BY event.id",
        )
        .bind(through)
        .bind(STREAM_REPLAY_TRANSITION_EVENTS)
        .bind(match stream {
            StreamName::Public
            | StreamName::PublicMedia
            | StreamName::PublicLocal
            | StreamName::PublicLocalMedia
            | StreamName::PublicRemote
            | StreamName::PublicRemoteMedia => "public",
            StreamName::Hashtag | StreamName::HashtagLocal => "hashtag",
            StreamName::List => "list",
            StreamName::User | StreamName::UserNotification | StreamName::Direct => unreachable!(),
        })
        .bind(local)
        .bind(remote)
        .bind(media)
        .bind(parameter)
        .bind(parameter.parse::<i64>().unwrap_or(0))
        .bind(account_id)
        .bind(create_after)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(stream_event_from_row).collect()
    }

    /// Reads retained timeline replay events with explicit limits for integration tests.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid bounds or when PostgreSQL rejects the read.
    pub async fn stream_replay_events_with_limits(
        &self,
        through: i64,
        transition_limit: i64,
        update_limit: i64,
    ) -> Result<Vec<StreamEvent>, JobError> {
        if through < 0 || transition_limit <= 0 || update_limit <= 0 {
            return Err(JobError::InvalidInput(
                "stream replay cursor must be non-negative and limits must be positive",
            ));
        }
        let rows = sqlx::query(
            "WITH selected AS ( \
               SELECT id FROM ( \
                 SELECT id FROM rustodon.outbox_events \
                  WHERE kind = 'rustodon.mastodon.stream_event' AND id <= $1 \
                    AND payload ->> 'account_id' = '0' \
                    AND payload ->> 'event' <> 'update' \
                  ORDER BY id DESC LIMIT $2 \
               ) transitions \
               UNION \
               SELECT id FROM ( \
                 SELECT id FROM rustodon.outbox_events \
                  WHERE kind = 'rustodon.mastodon.stream_event' AND id <= $1 \
                    AND payload ->> 'account_id' = '0' \
                    AND payload ->> 'event' = 'update' \
                  ORDER BY id DESC LIMIT $3 \
               ) updates \
             ) \
             SELECT event.id, (event.payload ->> 'account_id')::bigint AS account_id, \
                    event.payload ->> 'event' AS event, \
                    (event.payload ->> 'object_id')::bigint AS object_id, \
                    event.payload -> 'before' AS before, event.payload -> 'after' AS after \
               FROM rustodon.outbox_events event \
               JOIN selected ON selected.id = event.id \
              ORDER BY event.id",
        )
        .bind(through)
        .bind(transition_limit)
        .bind(update_limit)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(stream_event_from_row).collect()
    }

    /// Upserts a worker or scheduler heartbeat.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid heartbeat metadata or a rejected database operation.
    pub async fn heartbeat(&self, heartbeat: &WorkerHeartbeat) -> Result<(), JobError> {
        heartbeat.validate()?;
        let lanes = heartbeat
            .lanes
            .iter()
            .map(|lane| lane.as_str())
            .collect::<Vec<_>>();
        sqlx::query(
            "INSERT INTO rustodon.heartbeats (process_id, role, lanes, info) \
             VALUES ($1, $2, $3, $4) \
             ON CONFLICT (process_id) DO UPDATE \
             SET role = EXCLUDED.role, lanes = EXCLUDED.lanes, info = EXCLUDED.info, \
                 heartbeat_at = clock_timestamp()",
        )
        .bind(&heartbeat.process_id)
        .bind(heartbeat.role)
        .bind(&lanes)
        .bind(&heartbeat.info)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Removes a process heartbeat during graceful shutdown.
    ///
    /// # Errors
    ///
    /// Returns an error when `PostgreSQL` rejects the deletion.
    pub async fn remove_heartbeat(&self, process_id: &str) -> Result<(), JobError> {
        sqlx::query("DELETE FROM rustodon.heartbeats WHERE process_id = $1")
            .bind(process_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Reports live lane coverage, scheduler liveness, queue depth, and dead letters.
    ///
    /// # Errors
    ///
    /// Returns an error when `PostgreSQL` cannot inspect operational state.
    pub async fn readiness(
        &self,
        required_lanes: &BTreeSet<Lane>,
        freshness: Duration,
    ) -> Result<Readiness, JobError> {
        if freshness <= Duration::zero() {
            return Err(JobError::InvalidInput(
                "heartbeat freshness must be positive",
            ));
        }
        let rows = sqlx::query(
            "SELECT role, lanes FROM rustodon.heartbeats \
             WHERE heartbeat_at >= clock_timestamp() \
               - make_interval(secs => $1::double precision / 1000)",
        )
        .bind(freshness.num_milliseconds())
        .fetch_all(&self.pool)
        .await?;
        let mut covered = BTreeSet::new();
        let mut scheduler_alive = false;
        for row in rows {
            let role: String = row.try_get("role")?;
            if role == "scheduler" {
                scheduler_alive = true;
            } else if role == "worker" {
                for lane in row.try_get::<Vec<String>, _>("lanes")? {
                    covered.insert(lane.parse()?);
                }
            }
        }
        let summary = sqlx::query(
            "SELECT count(*) FILTER (WHERE dead_at IS NOT NULL) AS dead_letters, \
                    count(*) FILTER (WHERE dead_at IS NULL) AS queued_jobs, \
                    min(created_at) FILTER (WHERE dead_at IS NULL) AS oldest_queued_at \
             FROM rustodon.durable_jobs",
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(Readiness {
            missing_lanes: required_lanes.difference(&covered).copied().collect(),
            scheduler_alive,
            dead_letters: summary.try_get("dead_letters")?,
            queued_jobs: summary.try_get("queued_jobs")?,
            oldest_queued_at: summary.try_get("oldest_queued_at")?,
        })
    }

    /// Returns bounded dead-letter metadata without job arguments.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid limit or failed database query.
    pub async fn dead_letters(&self, limit: i64) -> Result<Vec<DeadLetter>, JobError> {
        if !(1..=1000).contains(&limit) {
            return Err(JobError::InvalidInput(
                "dead-letter limit must be between 1 and 1000",
            ));
        }
        sqlx::query(
            "SELECT id, lane, kind, attempts, max_attempts, last_error, dead_at \
             FROM rustodon.durable_jobs WHERE dead_at IS NOT NULL \
             ORDER BY dead_at DESC, id DESC LIMIT $1",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(|row| {
            Ok(DeadLetter {
                id: row.try_get("id")?,
                lane: row.try_get::<String, _>("lane")?.parse()?,
                kind: row.try_get("kind")?,
                attempts: row.try_get("attempts")?,
                max_attempts: row.try_get("max_attempts")?,
                last_error: row.try_get("last_error")?,
                dead_at: row.try_get("dead_at")?,
            })
        })
        .collect()
    }

    /// Counts live queued or leased jobs.
    ///
    /// # Errors
    ///
    /// Returns an error when `PostgreSQL` cannot inspect the queue.
    pub async fn queued_count(&self) -> Result<i64, JobError> {
        Ok(
            sqlx::query_scalar("SELECT count(*) FROM rustodon.durable_jobs WHERE dead_at IS NULL")
                .fetch_one(&self.pool)
                .await?,
        )
    }
}

/// Connects a writable operational pool with a deterministic safe session context.
///
/// # Errors
///
/// Returns an error when `PostgreSQL` cannot create or initialize the pool.
pub async fn connect_pool(
    options: PgConnectOptions,
    max_connections: u32,
) -> Result<PgPool, sqlx::Error> {
    PgPoolOptions::new()
        .max_connections(max_connections)
        .acquire_timeout(StdDuration::from_secs(10))
        .after_connect(|connection, _metadata| {
            Box::pin(async move {
                for setting in [
                    "SET TIME ZONE 'UTC'",
                    "SET search_path TO pg_catalog, public, rustodon, pg_temp",
                    "SET lock_timeout TO '10s'",
                    "SET statement_timeout TO '60s'",
                ] {
                    sqlx::query(setting).execute(&mut *connection).await?;
                }
                Ok(())
            })
        })
        .connect_with(options)
        .await
}

/// Enqueues a job inside an existing transaction for atomic application writes.
///
/// # Errors
///
/// Returns an error for invalid metadata or a rejected database operation.
pub async fn enqueue_in(
    transaction: &mut Transaction<'_, Postgres>,
    spec: &JobSpec,
) -> Result<i64, JobError> {
    spec.validate()?;
    Ok(sqlx::query_scalar(
        "INSERT INTO rustodon.durable_jobs \
           (lane, kind, arguments, logical_key, run_at, max_attempts) \
         VALUES ($1, $2, $3, $4, $5, $6) \
         ON CONFLICT (kind, logical_key) \
           WHERE logical_key IS NOT NULL AND dead_at IS NULL \
         DO UPDATE SET updated_at = rustodon.durable_jobs.updated_at \
         RETURNING id",
    )
    .bind(spec.lane.as_str())
    .bind(&spec.kind)
    .bind(&spec.arguments)
    .bind(&spec.logical_key)
    .bind(spec.run_at)
    .bind(spec.max_attempts)
    .fetch_one(&mut **transaction)
    .await?)
}

async fn enqueue_ordered_in(
    transaction: &mut Transaction<'_, Postgres>,
    spec: &JobSpec,
    ordering_key: &[u8; 32],
) -> Result<i64, JobError> {
    enqueue_ordered_kind_in(
        transaction,
        spec,
        ordering_key,
        ACTIVITYPUB_INBOX_ORDERING_KIND,
    )
    .await
}

async fn enqueue_ordered_kind_in(
    transaction: &mut Transaction<'_, Postgres>,
    spec: &JobSpec,
    ordering_key: &[u8; 32],
    ordering_kind: &str,
) -> Result<i64, JobError> {
    sqlx::query(
        "INSERT INTO rustodon.ordering_markers \
             (kind, key_hash, ordering_at, payload, created_at, expires_at) \
          VALUES ($1, $2, $3, '{}'::jsonb, clock_timestamp(), \
                  GREATEST(clock_timestamp() + interval '30 days', $3 + interval '30 days')) \
          ON CONFLICT (kind, key_hash) DO NOTHING",
    )
    .bind(ordering_kind)
    .bind(ordering_key.as_slice())
    .bind(spec.run_at_value())
    .execute(&mut **transaction)
    .await?;
    let previous = sqlx::query_as::<_, (DateTime<Utc>, Option<i64>)>(
        "SELECT ordering_at, (payload ->> 'job_id')::bigint AS previous_job_id \
           FROM rustodon.ordering_markers \
          WHERE kind = $1 AND key_hash = $2 \
          FOR UPDATE",
    )
    .bind(ordering_kind)
    .bind(ordering_key.as_slice())
    .fetch_optional(&mut **transaction)
    .await?;
    let predecessor_id = previous.as_ref().and_then(|previous| previous.1);
    let run_at = previous
        .as_ref()
        .filter(|(_, predecessor_id)| predecessor_id.is_some())
        .map_or(spec.run_at_value(), |previous| {
            std::cmp::max(spec.run_at_value(), previous.0 + Duration::microseconds(1))
        });
    let ordered_spec = with_ordering_metadata(spec, ordering_key, predecessor_id, run_at)?;
    let id = enqueue_in(transaction, &ordered_spec).await?;
    sqlx::query(
        "INSERT INTO rustodon.ordering_markers \
            (kind, key_hash, ordering_at, payload, created_at, expires_at) \
         VALUES ($1, $2, $3, jsonb_build_object('job_id', $4), \
                 clock_timestamp(), \
                 GREATEST(clock_timestamp() + interval '30 days', $3 + interval '30 days')) \
         ON CONFLICT (kind, key_hash) DO UPDATE SET \
            ordering_at = EXCLUDED.ordering_at, payload = EXCLUDED.payload, \
            expires_at = EXCLUDED.expires_at",
    )
    .bind(ordering_kind)
    .bind(ordering_key.as_slice())
    .bind(run_at)
    .bind(id)
    .execute(&mut **transaction)
    .await?;
    Ok(id)
}

fn with_ordering_metadata(
    spec: &JobSpec,
    ordering_key: &[u8; 32],
    predecessor_id: Option<i64>,
    run_at: DateTime<Utc>,
) -> Result<JobSpec, JobError> {
    let Value::Object(mut arguments) = spec.arguments.clone() else {
        return Err(JobError::InvalidInput(
            "ordered job arguments must be an object",
        ));
    };
    arguments.insert(
        "_rustodon_ordering_key".to_owned(),
        Value::String(hex(ordering_key)),
    );
    if let Some(predecessor_id) = predecessor_id {
        arguments.insert(
            "_rustodon_ordering_predecessor".to_owned(),
            json!(predecessor_id),
        );
    } else {
        arguments.remove("_rustodon_ordering_predecessor");
    }
    Ok(JobSpec {
        arguments: Value::Object(arguments),
        ..spec.clone().run_at(run_at)
    })
}

fn activitypub_delivery_ordering_key(spec: &JobSpec) -> Option<[u8; 32]> {
    if spec.kind != ACTIVITYPUB_DELIVERY_JOB_KIND {
        return None;
    }
    let arguments = spec.arguments.as_object()?;
    let source_account_id = arguments.get("source_account_id")?.as_i64()?;
    let inbox_url = arguments.get("inbox_url")?.as_str()?.trim();
    if inbox_url.is_empty() {
        return None;
    }
    Some(
        Sha256::digest(format!("activitypub-delivery:{source_account_id}:{inbox_url}").as_bytes())
            .into(),
    )
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut value, "{byte:02x}").expect("writing to a String cannot fail");
    }
    value
}

async fn enqueue_outbox_in(
    transaction: &mut Transaction<'_, Postgres>,
    spec: &JobSpec,
) -> Result<bool, JobError> {
    spec.validate()?;
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO rustodon.durable_jobs \
           (lane, kind, arguments, logical_key, run_at, max_attempts) \
         VALUES ($1, $2, $3, $4, $5, $6) \
         ON CONFLICT (kind, logical_key) \
           WHERE logical_key IS NOT NULL AND dead_at IS NULL \
         DO NOTHING RETURNING id",
    )
    .bind(spec.lane.as_str())
    .bind(&spec.kind)
    .bind(&spec.arguments)
    .bind(&spec.logical_key)
    .bind(spec.run_at)
    .bind(spec.max_attempts)
    .fetch_optional(&mut **transaction)
    .await?;
    Ok(id.is_some())
}

/// Records a job-shaped transactional outbox event inside an application transaction.
///
/// # Errors
///
/// Returns an error for invalid metadata or a rejected database operation.
pub async fn record_outbox_in(
    transaction: &mut Transaction<'_, Postgres>,
    spec: &JobSpec,
) -> Result<i64, JobError> {
    spec.validate()?;
    let payload = json!({
        "lane": spec.lane.as_str(),
        "arguments": spec.arguments,
        "run_at": spec.run_at.to_rfc3339(),
        "max_attempts": spec.max_attempts,
    });
    Ok(sqlx::query_scalar(
        "INSERT INTO rustodon.outbox_events (kind, logical_key, payload) \
         VALUES ($1, $2, $3) \
         ON CONFLICT (kind, logical_key) WHERE logical_key IS NOT NULL \
         DO UPDATE SET payload = EXCLUDED.payload, dispatched_at = NULL, \
                       created_at = clock_timestamp() \
         RETURNING id",
    )
    .bind(&spec.kind)
    .bind(&spec.logical_key)
    .bind(payload)
    .fetch_one(&mut **transaction)
    .await?)
}

/// Records a durable outbox event only once for its logical key.
///
/// This variant is for immutable activities whose already-dispatched payload must not be reset by
/// a retrying fan-out job.
///
/// # Errors
///
/// Returns an error for invalid metadata or a rejected database operation.
pub async fn record_outbox_once_in(
    transaction: &mut Transaction<'_, Postgres>,
    spec: &JobSpec,
) -> Result<bool, JobError> {
    spec.validate()?;
    let payload = json!({
        "lane": spec.lane.as_str(),
        "arguments": spec.arguments,
        "run_at": spec.run_at.to_rfc3339(),
        "max_attempts": spec.max_attempts,
    });
    Ok(sqlx::query_scalar::<_, i64>(
        "INSERT INTO rustodon.outbox_events (kind, logical_key, payload) \
          VALUES ($1, $2, $3) \
          ON CONFLICT (kind, logical_key) WHERE logical_key IS NOT NULL \
          DO NOTHING RETURNING id",
    )
    .bind(&spec.kind)
    .bind(&spec.logical_key)
    .bind(payload)
    .fetch_optional(&mut **transaction)
    .await?
    .is_some())
}

/// Records one immutable Mastodon stream event in the application transaction.
///
/// Stream rows are marked dispatched when inserted and use dedicated partial indexes. The
/// durable-job dispatcher also excludes [`STREAM_EVENT_KIND`], so it never consumes them.
///
/// # Errors
///
/// Returns an error for invalid event metadata or a rejected database operation.
pub async fn record_stream_event_in(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
    event: &str,
    object_id: i64,
    logical_key: &str,
) -> Result<i64, JobError> {
    if account_id <= 0 {
        return Err(JobError::InvalidInput(
            "stream event account ID must be positive",
        ));
    }
    record_stream_event_for_audience_in(
        transaction,
        account_id,
        event,
        object_id,
        logical_key,
        None,
        None,
    )
    .await
}

/// Records one audience-independent status lifecycle event using its current routing facts.
/// Production mutation paths should use [`record_global_stream_transition_in`] when facts changed.
///
/// # Errors
///
/// Returns an error for invalid event metadata or a rejected database operation.
pub async fn record_global_stream_event_in(
    transaction: &mut Transaction<'_, Postgres>,
    event: &str,
    object_id: i64,
    logical_key: &str,
) -> Result<i64, JobError> {
    let (had_media, language, public, hashtag, local) =
        sqlx::query_as::<_, (bool, Option<String>, bool, bool, bool)>(
            "SELECT EXISTS (SELECT 1 FROM media_attachments WHERE status_id = status.id), \
                status.language, \
                status.visibility = 0 AND author.suspended_at IS NULL \
                  AND author.silenced_at IS NULL AND status.reblog_of_id IS NULL \
                  AND (NOT status.reply OR status.in_reply_to_account_id = status.account_id), \
                status.visibility = 0 AND author.suspended_at IS NULL \
                  AND author.silenced_at IS NULL, \
                status.local OR status.uri IS NULL \
           FROM statuses status JOIN accounts author ON author.id = status.account_id \
          WHERE status.id = $1",
        )
        .bind(object_id)
        .fetch_one(&mut **transaction)
        .await?;
    let tags = sqlx::query_scalar::<_, String>(
        "SELECT lower(tag.name) FROM statuses_tags status_tag \
         JOIN tags tag ON tag.id = status_tag.tag_id WHERE status_tag.status_id = $1 \
         ORDER BY lower(tag.name)",
    )
    .bind(object_id)
    .fetch_all(&mut **transaction)
    .await?;
    let snapshot = TimelineRouteSnapshot {
        public,
        hashtag,
        local,
        had_media,
        language,
        tags,
        lists: Vec::new(),
    };
    let (before, after) = match event {
        "update" => (None, Some(snapshot)),
        "delete" => (Some(snapshot), None),
        _ => (Some(snapshot.clone()), Some(snapshot)),
    };
    record_global_stream_transition_in(
        transaction,
        event,
        object_id,
        logical_key,
        before.as_ref(),
        after.as_ref(),
    )
    .await
}

/// Records one audience-independent status lifecycle transition.
///
/// # Errors
///
/// Returns an error for invalid event metadata or a rejected database operation.
pub async fn record_global_stream_transition_in(
    transaction: &mut Transaction<'_, Postgres>,
    event: &str,
    object_id: i64,
    logical_key: &str,
    before: Option<&TimelineRouteSnapshot>,
    after: Option<&TimelineRouteSnapshot>,
) -> Result<i64, JobError> {
    record_stream_event_for_audience_in(
        transaction,
        0,
        event,
        object_id,
        logical_key,
        before,
        after,
    )
    .await
}

#[derive(Clone, Debug)]
pub(crate) struct PendingStreamEvent {
    account_id: i64,
    event: String,
    object_id: i64,
    logical_key: String,
    before: Option<TimelineRouteSnapshot>,
    after: Option<TimelineRouteSnapshot>,
}

#[cfg(feature = "test-support")]
#[derive(Debug, Default)]
pub struct StreamEventStagingProbe {
    events: Vec<PendingStreamEvent>,
}

#[cfg(feature = "test-support")]
impl StreamEventStagingProbe {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds one event to the probe buffer.
    ///
    /// # Errors
    ///
    /// Returns an error when the event metadata is invalid.
    pub fn push(
        &mut self,
        account_id: i64,
        event: &str,
        object_id: i64,
        logical_key: &str,
    ) -> Result<(), JobError> {
        self.events.push(pending_stream_event(
            account_id,
            event,
            object_id,
            logical_key,
            None,
            None,
        )?);
        Ok(())
    }

    /// Moves the current batch to transaction-local PostgreSQL staging without taking the global
    /// stream writer-order lock.
    ///
    /// # Errors
    ///
    /// Returns an error when PostgreSQL rejects staging.
    pub async fn stage(
        &mut self,
        transaction: &mut Transaction<'_, Postgres>,
    ) -> Result<(), JobError> {
        stage_stream_events_in(transaction, &mut self.events).await
    }

    /// Performs the terminal ordered flush under the global stream writer-order lock.
    ///
    /// # Errors
    ///
    /// Returns an error when PostgreSQL rejects the flush.
    pub async fn flush(
        &mut self,
        transaction: &mut Transaction<'_, Postgres>,
    ) -> Result<(), JobError> {
        flush_staged_stream_events_in(transaction, &mut self.events).await
    }
}

pub(crate) fn pending_stream_event(
    account_id: i64,
    event: &str,
    object_id: i64,
    logical_key: &str,
    before: Option<&TimelineRouteSnapshot>,
    after: Option<&TimelineRouteSnapshot>,
) -> Result<PendingStreamEvent, JobError> {
    if account_id < 0 {
        return Err(JobError::InvalidInput(
            "stream event account ID must be non-negative",
        ));
    }
    if object_id == 0 {
        return Err(JobError::InvalidInput(
            "stream event object ID must be non-zero",
        ));
    }
    if !(1..=128).contains(&event.len()) {
        return Err(JobError::InvalidInput(
            "stream event name must contain 1-128 bytes",
        ));
    }
    if !(1..=1024).contains(&logical_key.len()) {
        return Err(JobError::InvalidInput(
            "stream event logical key must contain 1-1024 bytes",
        ));
    }
    Ok(PendingStreamEvent {
        account_id,
        event: event.to_owned(),
        object_id,
        logical_key: logical_key.to_owned(),
        before: before.cloned(),
        after: after.cloned(),
    })
}

const PENDING_STREAM_EVENT_STAGE_SIZE: usize = 256;

pub(crate) async fn stage_stream_events_if_large_in(
    transaction: &mut Transaction<'_, Postgres>,
    events: &mut Vec<PendingStreamEvent>,
) -> Result<(), JobError> {
    if events.len() >= PENDING_STREAM_EVENT_STAGE_SIZE {
        stage_stream_events_in(transaction, events).await?;
    }
    Ok(())
}

pub(crate) async fn stage_stream_events_in(
    transaction: &mut Transaction<'_, Postgres>,
    events: &mut Vec<PendingStreamEvent>,
) -> Result<(), JobError> {
    for event in events.drain(..) {
        sqlx::query(
            "INSERT INTO rustodon.outbox_events \
                 (kind, logical_key, payload, dispatched_at) \
             VALUES ($1, $2, $3, clock_timestamp()) \
             ON CONFLICT (kind, logical_key) WHERE logical_key IS NOT NULL DO NOTHING",
        )
        .bind(STREAM_EVENT_STAGING_KIND)
        .bind(&event.logical_key)
        .bind(stream_event_payload(&event))
        .execute(&mut **transaction)
        .await?;
    }
    Ok(())
}

async fn has_staged_stream_events_in(
    transaction: &mut Transaction<'_, Postgres>,
) -> Result<bool, JobError> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM rustodon.outbox_events WHERE kind = $1)",
    )
    .bind(STREAM_EVENT_STAGING_KIND)
    .fetch_one(&mut **transaction)
    .await?)
}

pub(crate) async fn flush_staged_stream_events_in(
    transaction: &mut Transaction<'_, Postgres>,
    events: &mut Vec<PendingStreamEvent>,
) -> Result<(), JobError> {
    stage_stream_events_in(transaction, events).await?;
    if !has_staged_stream_events_in(transaction).await? {
        return Ok(());
    }
    lock_stream_event_order(transaction).await?;
    sqlx::query(
        "INSERT INTO rustodon.outbox_events \
             (kind, logical_key, payload, dispatched_at) \
         SELECT $1, logical_key, payload, clock_timestamp() \
           FROM rustodon.outbox_events WHERE kind = $2 ORDER BY id \
         ON CONFLICT (kind, logical_key) WHERE logical_key IS NOT NULL DO NOTHING",
    )
    .bind(STREAM_EVENT_KIND)
    .bind(STREAM_EVENT_STAGING_KIND)
    .execute(&mut **transaction)
    .await?;
    sqlx::query("DELETE FROM rustodon.outbox_events WHERE kind = $1")
        .bind(STREAM_EVENT_STAGING_KIND)
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

pub(crate) async fn flush_stream_events_in(
    transaction: &mut Transaction<'_, Postgres>,
    events: &mut Vec<PendingStreamEvent>,
) -> Result<Vec<i64>, JobError> {
    if has_staged_stream_events_in(transaction).await? {
        flush_staged_stream_events_in(transaction, events).await?;
        return Ok(Vec::new());
    }
    if events.is_empty() {
        return Ok(Vec::new());
    }
    lock_stream_event_order(transaction).await?;
    let mut ids = Vec::with_capacity(events.len());
    for event in events.drain(..) {
        ids.push(insert_pending_stream_event_in(transaction, &event).await?);
    }
    Ok(ids)
}

fn stream_event_payload(event: &PendingStreamEvent) -> Value {
    json!({
        "account_id": event.account_id,
        "event": event.event,
        "object_id": event.object_id,
        "before": event.before,
        "after": event.after,
    })
}

async fn insert_pending_stream_event_in(
    transaction: &mut Transaction<'_, Postgres>,
    event: &PendingStreamEvent,
) -> Result<i64, JobError> {
    let payload = stream_event_payload(event);
    if let Some(id) = sqlx::query_scalar::<_, i64>(
        "INSERT INTO rustodon.outbox_events (kind, logical_key, payload, dispatched_at) \
          VALUES ($1, $2, $3, clock_timestamp()) \
          ON CONFLICT (kind, logical_key) WHERE logical_key IS NOT NULL \
          DO NOTHING RETURNING id",
    )
    .bind(STREAM_EVENT_KIND)
    .bind(&event.logical_key)
    .bind(payload)
    .fetch_optional(&mut **transaction)
    .await?
    {
        return Ok(id);
    }
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT id FROM rustodon.outbox_events WHERE kind = $1 AND logical_key = $2",
    )
    .bind(STREAM_EVENT_KIND)
    .bind(&event.logical_key)
    .fetch_one(&mut **transaction)
    .await?)
}

async fn record_stream_event_for_audience_in(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
    event: &str,
    object_id: i64,
    logical_key: &str,
    before: Option<&TimelineRouteSnapshot>,
    after: Option<&TimelineRouteSnapshot>,
) -> Result<i64, JobError> {
    let mut events = vec![pending_stream_event(
        account_id,
        event,
        object_id,
        logical_key,
        before,
        after,
    )?];
    flush_stream_events_in(transaction, &mut events)
        .await?
        .pop()
        .ok_or(JobError::InvalidData("stream event batch was empty"))
}

fn stream_event_from_row(row: sqlx::postgres::PgRow) -> Result<StreamEvent, JobError> {
    let decode_snapshot = |column| -> Result<Option<TimelineRouteSnapshot>, JobError> {
        let value = row.try_get::<Option<Value>, _>(column)?;
        value
            .filter(|value| !value.is_null())
            .map(serde_json::from_value)
            .transpose()
            .map_err(|_| JobError::InvalidData("stream route snapshot is invalid"))
    };
    Ok(StreamEvent {
        id: row.try_get("id")?,
        account_id: row.try_get("account_id")?,
        event: row.try_get("event")?,
        object_id: row.try_get("object_id")?,
        before: decode_snapshot("before")?,
        after: decode_snapshot("after")?,
    })
}

async fn lock_stream_event_order(
    transaction: &mut Transaction<'_, Postgres>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "SELECT pg_catalog.pg_advisory_xact_lock(
            pg_catalog.hashtextextended($1, 0)
         )",
    )
    .bind(STREAM_EVENT_ORDERING_LOCK_KEY)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

fn claimed_job(row: &sqlx::postgres::PgRow) -> Result<ClaimedJob, JobError> {
    Ok(ClaimedJob {
        id: row.try_get("id")?,
        lane: row.try_get::<String, _>("lane")?.parse()?,
        kind: row.try_get("kind")?,
        arguments: row.try_get("arguments")?,
        logical_key: row.try_get("logical_key")?,
        run_at: row.try_get("run_at")?,
        attempt: row.try_get("attempts")?,
        max_attempts: row.try_get("max_attempts")?,
        generation: row.try_get("lease_generation")?,
        lease_owner: row.try_get("lease_owner")?,
        lease_expires_at: row.try_get("lease_expires_at")?,
    })
}

fn outbox_spec(
    kind: String,
    logical_key: Option<String>,
    payload: &Value,
) -> Result<JobSpec, JobError> {
    let object = payload
        .as_object()
        .ok_or(JobError::InvalidData("outbox payload must be an object"))?;
    let lane = object
        .get("lane")
        .and_then(Value::as_str)
        .ok_or(JobError::InvalidData("outbox lane is missing"))?
        .parse()?;
    let arguments = object
        .get("arguments")
        .cloned()
        .ok_or(JobError::InvalidData("outbox arguments are missing"))?;
    let run_at = object
        .get("run_at")
        .and_then(Value::as_str)
        .ok_or(JobError::InvalidData("outbox run_at is missing"))?
        .parse::<DateTime<Utc>>()
        .map_err(|_| JobError::InvalidData("outbox run_at is invalid"))?;
    let max_attempts = object
        .get("max_attempts")
        .and_then(Value::as_i64)
        .and_then(|value| i32::try_from(value).ok())
        .ok_or(JobError::InvalidData("outbox max_attempts is invalid"))?;
    let mut spec = JobSpec::new(lane, kind, arguments)
        .run_at(run_at)
        .max_attempts(max_attempts);
    spec.logical_key = logical_key;
    spec.validate()?;
    Ok(spec)
}

fn validate_lease(lease_owner: &str, duration: Duration) -> Result<(), JobError> {
    if !(1..=255).contains(&lease_owner.len()) {
        return Err(JobError::InvalidInput(
            "lease owner must contain 1-255 bytes",
        ));
    }
    if duration <= Duration::zero() || duration.num_milliseconds() <= 0 {
        return Err(JobError::InvalidInput("lease duration must be positive"));
    }
    Ok(())
}

fn bounded(value: &str, max_bytes: usize) -> &str {
    if value.len() <= max_bytes {
        return value;
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}
