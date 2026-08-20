use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, RwLock};
use std::time::Duration as StdDuration;

use chrono::{Duration, Utc};
use serde_json::json;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, watch};
use tokio::task::JoinSet;

use crate::config::WorkerConfig;
use crate::jobs::{ClaimedJob, JobError, JobSpec, Lane, Queue, WorkerHeartbeat};

type HandlerFuture = Pin<Box<dyn Future<Output = Result<(), HandlerFailure>> + Send>>;
type HandlerFn = dyn Fn(ClaimedJob) -> HandlerFuture + Send + Sync;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResourceClass {
    None,
    RemoteHttp,
    Media,
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

/// Builds the infrastructure handlers available before feature-specific handlers are registered.
///
/// # Errors
///
/// Returns an error if a built-in handler cannot be registered.
pub fn infrastructure_handlers(queue: &Queue) -> Result<HandlerRegistry, WorkerError> {
    let handlers = HandlerRegistry::new();
    let pool = queue.pool().clone();
    handlers.register(
        "rustodon.maintenance.prune",
        Lane::Maintenance,
        ResourceClass::None,
        move |_job| {
            let pool = pool.clone();
            async move {
                sqlx::raw_sql(
                    "DELETE FROM rustodon.idempotency_keys WHERE expires_at <= clock_timestamp(); \
                     DELETE FROM rustodon.ordering_markers WHERE expires_at <= clock_timestamp(); \
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
    use super::{HandlerRegistry, ResourceClass, retry_delay};
    use crate::jobs::Lane;

    #[test]
    fn retry_backoff_is_deterministic_jittered_and_bounded() {
        assert_eq!(retry_delay(42, 2), retry_delay(42, 2));
        assert_ne!(retry_delay(42, 2), retry_delay(43, 2));
        assert!(retry_delay(42, 1) < retry_delay(42, 2));
        assert!(retry_delay(42, 100).num_hours() <= 24);
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
}
