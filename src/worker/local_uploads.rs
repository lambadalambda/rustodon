//! Durable local upload processing and exact-manifest cleanup.
use std::io::Read;
use std::path::Path;

use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::PgPool;

use super::{HandlerFailure, HandlerRegistry, ResourceClass, WorkerError};
use crate::jobs::{ClaimedJob, JobSpec, Lane, Queue};
use crate::mastodon::{
    AccountProfileValue, MediaAttachmentCreate, WriteError, WriteRepository,
    local_uploads::{
        UploadIdentity, UploadState, abandon_in, claim_in, discard_staging_in, forget_orphan_in,
        load_in, publish_in, raw_path, register_outputs_in, retire_ready_in,
    },
};
use crate::paperclip::{
    PaperclipAttachment, PaperclipMetadata, PaperclipRoot, PreparedMediaAttachment,
    parse_paperclip_path, prepare_rich_media_attachment, write_prepared_media,
};

pub const PROCESS_KIND: &str = "rustodon.mastodon.process_local_upload";
pub const RECOVER_KIND: &str = "rustodon.mastodon.recover_local_uploads";

/// Worker-readiness gate for installations that accept local uploads. A generic
/// Maintenance worker without the local writer/root handlers is not sufficient.
/// # Errors
/// Returns database failures or an invalid freshness bound.
pub async fn ready(
    queue: &Queue,
    freshness: chrono::Duration,
) -> Result<bool, crate::jobs::JobError> {
    if freshness <= chrono::Duration::zero() {
        return Err(crate::jobs::JobError::InvalidInput(
            "heartbeat freshness must be positive",
        ));
    }
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM rustodon.heartbeats
         WHERE role = 'worker' AND 'maintenance' = ANY(lanes)
           AND info @> '{\"local_uploads\":true}'::jsonb
           AND heartbeat_at >= clock_timestamp() - make_interval(secs => $1::double precision / 1000))",
    ).bind(freshness.num_milliseconds()).fetch_one(queue.pool()).await?)
}

pub(super) async fn schedule_recovery(
    queue: &Queue,
) -> Result<crate::jobs::KindScheduleOwnership, crate::jobs::JobError> {
    queue
        .enqueue_if_kind_idle(
            RECOVER_KIND,
            &JobSpec::new(Lane::Maintenance, RECOVER_KIND, json!({}))
                .logical_key("local-upload-recovery:root")
                .max_attempts(1),
        )
        .await
}

#[must_use]
pub fn processing_job(id: UploadIdentity) -> JobSpec {
    JobSpec::new(
        Lane::Maintenance,
        PROCESS_KIND,
        json!({
            "media_id": id.media_id, "account_id": id.account_id, "generation": id.generation,
        }),
    )
    .logical_key(format!(
        "mastodon:media:{}:upload:{}",
        id.media_id, id.generation
    ))
    .max_attempts(4)
}

pub(super) fn register(
    handlers: &HandlerRegistry,
    pool: PgPool,
    queue: Queue,
    root: PaperclipRoot,
) -> Result<(), WorkerError> {
    let process_pool = pool.clone();
    let process_root = root.clone();
    handlers.register(
        PROCESS_KIND,
        Lane::Maintenance,
        ResourceClass::Media,
        move |job| {
            let pool = process_pool.clone();
            let root = process_root.clone();
            async move { process(pool, root, job).await }
        },
    )?;
    handlers.register(
        RECOVER_KIND,
        Lane::Maintenance,
        ResourceClass::Media,
        move |job| {
            let pool = pool.clone();
            let queue = queue.clone();
            let root = root.clone();
            async move { recover(pool, queue, root, &job.arguments).await }
        },
    )
}

fn identity(job: &ClaimedJob) -> Result<UploadIdentity, HandlerFailure> {
    let positive = |key| {
        job.arguments
            .get(key)
            .and_then(serde_json::Value::as_i64)
            .filter(|value| *value > 0)
            .ok_or_else(|| HandlerFailure::permanent("invalid local upload identity"))
    };
    Ok(UploadIdentity {
        media_id: positive("media_id")?,
        account_id: positive("account_id")?,
        generation: positive("generation")?,
    })
}

fn read_raw(root: &PaperclipRoot, state: &UploadState) -> Result<Vec<u8>, WriteError> {
    let id = UploadIdentity {
        media_id: state.media_id,
        account_id: state.account_id,
        generation: state.generation,
    };
    if state.raw_path != raw_path(id) || state.raw_size <= 0 || state.raw_size > 1_073_741_824 {
        return Err(WriteError::Validation("invalid raw upload manifest"));
    }
    let mut bytes = Vec::new();
    root.open_file(Path::new(&state.raw_path))?
        .take(state.raw_size.cast_unsigned() + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 != state.raw_size.cast_unsigned()
        || Sha256::digest(&bytes).as_slice() != state.raw_sha256
    {
        return Err(WriteError::Validation("raw upload size/hash mismatch"));
    }
    Ok(bytes)
}

async fn public_state<'a>(
    connection: impl sqlx::Executor<'a, Database = sqlx::Postgres>,
    id: UploadIdentity,
) -> Result<Option<(Option<i32>, Option<String>)>, WriteError> {
    // Any surviving public owner prevents orphan cleanup; retirement and claims
    // separately verify the expected account. Reuse the caller's transaction.
    Ok(sqlx::query_as(
        "SELECT processing, file_file_name FROM media_attachments WHERE id = $1 FOR UPDATE",
    )
    .bind(id.media_id)
    .fetch_optional(connection)
    .await?)
}

// The caller holds the account lock. Ready cleanup NEVER touches output files.
async fn cleanup_locked(
    pool: &PgPool,
    root: &PaperclipRoot,
    id: UploadIdentity,
) -> Result<bool, WriteError> {
    let mut tx = pool.begin().await?;
    let Some(state) = load_in(&mut tx, id).await? else {
        return Ok(true);
    };
    let public = public_state(&mut *tx, id).await?;
    let raw = root.private_upload_root()?;
    match public {
        Some((Some(2), Some(_))) => {
            // Validate public ownership before unlink; an ambiguous commit can only
            // remove this raw ownership record, never undo published output.
            retire_ready_in(&mut tx, id).await?;
            raw.remove_file(Path::new(&state.raw_path))?;
        }
        None | Some((Some(3), None)) => {
            if state.raw_path != raw_path(id) {
                return Err(WriteError::Conflict);
            }
            for path in &state.output_paths {
                let parsed = parse_paperclip_path(path).ok_or(WriteError::Conflict)?;
                if parsed.id() != id.media_id
                    || parsed.attachment() != PaperclipAttachment::MediaFile
                {
                    return Err(WriteError::Conflict);
                }
            }
            // Validate absence or exact failed ownership BEFORE destructive I/O.
            // Unlink errors roll this uncommitted retirement back for recovery.
            forget_orphan_in(&mut tx, id).await?;
            raw.remove_file(Path::new(&state.raw_path))?;
            for path in &state.output_paths {
                root.remove_file(Path::new(path))?;
            }
        }
        _ => return Ok(false),
    }
    tx.commit().await?;
    Ok(true)
}

fn create(prepared: &PreparedMediaAttachment) -> MediaAttachmentCreate {
    MediaAttachmentCreate {
        media_type: prepared.media_kind.database_type(),
        content_type: prepared.content_type.clone(),
        file_name: prepared.file_name.clone(),
        file_size: prepared.file_size,
        file_meta: prepared.file_meta.clone(),
        blurhash: prepared.blurhash.clone(),
        description: None,
        focus: AccountProfileValue::Unchanged,
    }
}

async fn process(pool: PgPool, root: PaperclipRoot, job: ClaimedJob) -> Result<(), HandlerFailure> {
    process_with(pool, root, job, |id, mime, bytes| async move {
        prepare_rich_media_attachment(id.account_id, "upload", &mime, &bytes).await
    })
    .await
}

async fn process_with<F, Fut>(
    pool: PgPool,
    root: PaperclipRoot,
    job: ClaimedJob,
    prepare: F,
) -> Result<(), HandlerFailure>
where
    F: FnOnce(UploadIdentity, String, Vec<u8>) -> Fut,
    Fut: std::future::Future<
            Output = Result<PreparedMediaAttachment, crate::paperclip::MediaAttachmentError>,
        >,
{
    let id = identity(&job)?;
    let writer = WriteRepository::from_pool(pool.clone());
    let acquired = writer
        .with_account_lock(id.account_id, || async {
            if cleanup_locked(&pool, &root, id).await? {
                return Ok(None);
            }
            writer.ensure_account_write_allowed(id.account_id).await?;
            let mut tx = pool.begin().await?;
            let token = claim_in(&mut tx, id).await?;
            tx.commit().await?;
            let mut tx = pool.begin().await?;
            let state = load_in(&mut tx, id).await?.ok_or(WriteError::Conflict)?;
            if state.claim != token.claim {
                return Err(WriteError::Conflict);
            }
            let bytes = read_raw(&root.private_upload_root()?, &state)?;
            tx.rollback().await?;
            Ok(Some((token, state.raw_mime, bytes)))
        })
        .await
        .map_err(|_| HandlerFailure::retry("local upload claim/read failed"))?;
    let Some((token, mime, bytes)) = acquired else {
        return Ok(());
    };
    // No filesystem operations and no account lock during bounded child processing.
    let prepared = match prepare(id, mime, bytes).await {
        Ok(prepared) => prepared,
        Err(
            crate::paperclip::MediaAttachmentError::ProcessingUnavailable
            | crate::paperclip::MediaAttachmentError::ProcessingTimedOut,
        ) => {
            return Err(HandlerFailure::retry(
                "local upload processor unavailable or timed out",
            ));
        }
        Err(_) => {
            writer
                .with_account_lock(id.account_id, || async {
                    if cleanup_locked(&pool, &root, id).await? {
                        return Ok(());
                    }
                    let mut tx = pool.begin().await?;
                    abandon_in(&mut tx, token).await?;
                    tx.commit().await?;
                    cleanup_locked(&pool, &root, id).await?;
                    Ok(())
                })
                .await
                .map_err(|_| HandlerFailure::retry("local upload failure cleanup failed"))?;
            return Ok(());
        }
    };
    writer
        .with_account_lock(id.account_id, || async {
            if cleanup_locked(&pool, &root, id).await? {
                return Ok(());
            }
            writer.ensure_account_write_allowed(id.account_id).await?;
            let output = create(&prepared);
            let mut tx = pool.begin().await?;
            register_outputs_in(&mut tx, token, &output).await?;
            tx.commit().await?;
            // Reload even after an acknowledged commit; no rewrite based on an old
            // claim or an inferred rollback. Ambiguous publication returns untouched.
            if cleanup_locked(&pool, &root, id).await? {
                return Ok(());
            }
            let mut tx = pool.begin().await?;
            register_outputs_in(&mut tx, token, &output).await?;
            let state = load_in(&mut tx, id).await?.ok_or(WriteError::Conflict)?;
            tx.rollback().await?;
            for path in &state.output_paths {
                root.remove_file(Path::new(path))?;
            }
            let metadata = PaperclipMetadata {
                attachment: PaperclipAttachment::MediaFile,
                id: id.media_id,
                remote: false,
                storage_schema_version: Some(1),
                file_name: output.file_name.clone(),
                content_type: Some(output.content_type.clone()),
                variant: None,
            };
            write_prepared_media(&root, &metadata, &prepared)?;
            let mut tx = pool.begin().await?;
            publish_in(&mut tx, token, &output).await?;
            #[cfg(feature = "test-support")]
            if root.take_commit_before_fault() {
                return Err(WriteError::Conflict);
            }
            tx.commit().await?;
            #[cfg(feature = "test-support")]
            if root.take_commit_after_fault() {
                return Err(WriteError::Conflict);
            }
            cleanup_locked(&pool, &root, id).await?;
            Ok(())
        })
        .await
        .map_err(|_| HandlerFailure::retry("local upload installation failed"))
}

async fn recover(
    pool: PgPool,
    queue: Queue,
    root: PaperclipRoot,
    arguments: &serde_json::Value,
) -> Result<(), HandlerFailure> {
    let after = arguments
        .get("after")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0)
        .max(0);
    let rows: Vec<(i64, i64, i64, bool)> = sqlx::query_as(
        "SELECT media_id, account_id, generation, created_at < clock_timestamp() - interval '1 hour'
         FROM rustodon.local_uploads WHERE media_id > $1 ORDER BY media_id LIMIT 100")
        .bind(after).fetch_all(&pool).await.map_err(|_| HandlerFailure::retry("upload scan failed"))?;
    let writer = WriteRepository::from_pool(pool.clone());
    let mut failed = false;
    for &(media_id, account_id, generation, old) in &rows {
        let id = UploadIdentity {
            media_id,
            account_id,
            generation,
        };
        let key = format!("mastodon:media:{media_id}:upload:{generation}");
        // Runtime state is read through the queue role, never widened writer grants.
        failed |= writer.with_account_lock(account_id, || async {
            if cleanup_locked(&pool, &root, id).await? { return Ok(()); }
            let mut tx = pool.begin().await?;
            let Some(state) = load_in(&mut tx, id).await? else { return Ok(()); };
            if !old { return Ok(()); }
            if state.accepted {
                let undispatched: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM rustodon.outbox_events WHERE kind = $1 AND logical_key = $2 AND dispatched_at IS NULL)")
                    .bind(PROCESS_KIND).bind(&key).fetch_one(&mut *tx).await?;
                if undispatched { return Ok(()); }
        let active: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM rustodon.durable_jobs WHERE kind = $1 AND logical_key = $2 AND dead_at IS NULL)")
            .bind(PROCESS_KIND).bind(&key).fetch_one(queue.pool()).await?;
                if active { return Ok(()); }
                let token = claim_in(&mut tx, id).await?;
                abandon_in(&mut tx, token).await?;
            } else {
                discard_staging_in(&mut tx, id).await?;
            }
            tx.commit().await?;
            cleanup_locked(&pool, &root, id).await?;
            Ok(())
        }).await.is_err();
    }
    if rows.len() == 100 {
        queue
            .enqueue(
                &JobSpec::new(
                    Lane::Maintenance,
                    RECOVER_KIND,
                    json!({"after": rows.last().expect("nonempty").0}),
                )
                .logical_key(format!(
                    "local-upload-recovery:after:{}",
                    rows.last().expect("nonempty").0
                ))
                .max_attempts(1),
            )
            .await
            .map_err(|_| HandlerFailure::retry("upload scan continuation failed"))?;
    }
    if failed {
        return Err(HandlerFailure::retry(
            "upload recovery retained failed owners",
        ));
    }
    Ok(())
}

#[cfg(all(test, feature = "test-support"))]
mod tests;
