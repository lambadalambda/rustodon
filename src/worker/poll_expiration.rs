//! Poll expiration notifications: bounded startup and periodic reconciliation.

#[allow(clippy::wildcard_imports)] // shares the parent module namespace
use super::*;

pub(super) const POLL_EXPIRATION_REPAIR_PAGE_SIZE: i64 = 256;
pub(super) const POLL_EXPIRATION_REPAIR_MAX_PAGES: usize = 4;
pub(super) const POLL_EXPIRATION_REPAIR_MAX_CANDIDATES: usize = 25;
pub(super) const POLL_EXPIRATION_REPAIR_WALL_TIME: StdDuration = StdDuration::from_secs(2);
pub(super) const POLL_EXPIRATION_REPAIR_WORK_TIME: StdDuration = StdDuration::from_millis(1_500);
pub(super) const POLL_EXPIRATION_REPAIR_CONTINUATION_RESERVE: StdDuration =
    StdDuration::from_millis(500);
pub(super) const POLL_EXPIRATION_REPAIR_OPERATION_TIMEOUT: StdDuration = StdDuration::from_secs(1);
pub(super) const POLL_EXPIRATION_STARTUP_LEASE_DURATION: Duration = Duration::seconds(3);
pub(super) const POLL_EXPIRATION_STARTUP_SUCCESS_POLL_INTERVAL: StdDuration =
    StdDuration::from_millis(25);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PollExpirationScanMode {
    Optimized,
    Raw,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PollExpirationScanStep {
    AdvanceTerminal,
    Reconcile,
    Stop,
}

pub(super) fn poll_expiration_scan_step(
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

pub(super) async fn within_poll_expiration_reconciliation_wall_time<F>(
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

pub(super) fn poll_expiration_reconciliation_job(
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

pub(super) fn poll_expiration_raw_reconciliation_job(
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

pub(super) fn poll_expiration_reconciliation_continuation(
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

pub(super) async fn enqueue_poll_expiration_reconciliation_continuation(
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

pub(super) fn poll_expiration_reconciliation_job_with_mode(
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

pub(super) fn poll_expiration_reconciliation_logical_key(
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

pub(super) fn parse_poll_expiration_scan_mode(
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

pub(super) fn poll_expiration_no_progress() -> HandlerFailure {
    HandlerFailure::retry("poll expiration reconciliation made no cursor progress")
}

pub(super) fn poll_expiration_execution_mode(
    configured_mode: PollExpirationScanMode,
    attempt: i32,
) -> PollExpirationScanMode {
    if configured_mode == PollExpirationScanMode::Raw || attempt > 1 {
        PollExpirationScanMode::Raw
    } else {
        PollExpirationScanMode::Optimized
    }
}

pub(super) fn poll_expiration_reconciliation_key_from_arguments(
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

pub(super) fn poll_expiration_scan_timed_out(error: &sqlx::Error) -> bool {
    matches!(
        error,
        sqlx::Error::Database(database) if database.code().as_deref() == Some("57014")
    )
}

pub(super) type PollExpirationScanRow = (
    i64,
    NaiveDateTime,
    bool,
    Option<(Value, Option<DateTime<Utc>>)>,
);

#[derive(Debug)]
pub(super) enum PollExpirationHighWater {
    Value(i64),
    TimedOut,
}

pub(super) async fn poll_expiration_high_water(
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
pub(super) enum PollExpirationOptimizedPage {
    Rows(Vec<PollExpirationScanRow>),
    TimedOut,
}

pub(super) fn poll_expiration_statement_timeout_value(limit: StdDuration) -> String {
    format!("{}ms", limit.as_millis().max(1))
}

#[allow(clippy::too_many_lines)]
pub(super) async fn poll_expiration_optimized_page(
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
pub(super) enum PollExpirationFallbackPageError {
    TimedOut,
    Failure(HandlerFailure),
}

#[allow(clippy::too_many_lines)]
pub(super) async fn poll_expiration_fallback_page(
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

pub(super) async fn poll_expiration_fallback_page_within_work_time(
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

pub(super) async fn reconcile_poll_expirations(
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
pub(super) async fn reconcile_poll_expirations_inner(
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
