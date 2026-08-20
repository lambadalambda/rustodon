use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Duration, Utc};
use serde_json::{Value, json};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{PgPool, Postgres, Row, Transaction};
use std::time::Duration as StdDuration;

const DEFAULT_MAX_ATTEMPTS: i32 = 25;
const MAX_ERROR_BYTES: usize = 4 * 1024;

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
    pub const fn max_attempts(mut self, max_attempts: i32) -> Self {
        self.max_attempts = max_attempts;
        self
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
    InvalidInput(&'static str),
    InvalidData(&'static str),
}

impl fmt::Display for JobError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlx(_) => formatter.write_str("PostgreSQL rejected a durable-job operation"),
            Self::InvalidInput(message) | Self::InvalidData(message) => {
                formatter.write_str(message)
            }
        }
    }
}

impl std::error::Error for JobError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Sqlx(error) => Some(error),
            Self::InvalidInput(_) | Self::InvalidData(_) => None,
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
}

impl Queue {
    #[must_use]
    pub const fn new(pool: PgPool) -> Self {
        Self { pool }
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
        let row = sqlx::query(
            "WITH candidate AS ( \
               SELECT id FROM rustodon.durable_jobs \
               WHERE dead_at IS NULL AND run_at <= clock_timestamp() \
                 AND (lease_expires_at IS NULL OR lease_expires_at <= clock_timestamp()) \
                 AND lane = ANY($1) \
               ORDER BY run_at, id FOR UPDATE SKIP LOCKED LIMIT 1) \
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
               AND (event.logical_key IS NULL OR NOT EXISTS ( \
                 SELECT 1 FROM rustodon.durable_jobs job \
                 WHERE job.kind = event.kind AND job.logical_key = event.logical_key \
                   AND job.dead_at IS NULL)) \
             ORDER BY event.id FOR UPDATE OF event SKIP LOCKED LIMIT $1",
        )
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
            if enqueue_outbox_in(&mut transaction, &spec).await? {
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
