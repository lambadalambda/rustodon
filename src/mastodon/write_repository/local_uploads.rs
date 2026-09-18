//! Durable ownership only: no filesystem I/O, worker registration, or HTTP callers.
//!
//! Callers must hold `WriteRepository::with_account_lock` across each transaction and
//! any associated filesystem writes. Row/claim fencing alone cannot fence a filesystem
//! writer. Commit staging/manifest transactions *before* creating the named files.
//! On ambiguous commit, reload ownership; never infer rollback from a transport error.

use serde_json::{Value, json};
use sqlx::{Postgres, Transaction};

use super::{MediaAttachmentCreate, media_format, validate_media_attachment_create};
use crate::jobs::{JobSpec, record_outbox_once_in};
use crate::mastodon::WriteError;
use crate::paperclip::{PaperclipAttachment, PaperclipMetadata};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UploadIdentity {
    pub media_id: i64,
    pub account_id: i64,
    pub generation: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UploadClaim {
    pub identity: UploadIdentity,
    pub claim: i64,
}

pub struct RawInput<'a> {
    pub mime: &'a str,
    pub size: i64,
    pub sha256: &'a [u8; 32],
}

#[derive(Debug, sqlx::FromRow)]
pub struct UploadState {
    pub media_id: i64,
    pub account_id: i64,
    pub generation: i64,
    pub accepted: bool,
    pub claim: i64,
    pub raw_mime: String,
    pub raw_size: i64,
    pub raw_sha256: Vec<u8>,
    pub raw_path: String,
    pub output_paths: Vec<String>,
}

/// Stage a new private pending media row and its raw cleanup owner atomically.
/// This deliberately does NOT use legacy `stage_media_attachment` or its rollback job.
/// Account authorization/lifecycle locking belongs to the caller, just as for other
/// transaction-level repository primitives. No bytes may be written until commit.
///
/// # Errors
/// Rejects invalid identity/input bounds and database failures.
pub async fn stage_in(
    tx: &mut Transaction<'_, Postgres>,
    account_id: i64,
    generation: i64,
    input: &RawInput<'_>,
) -> Result<UploadIdentity, WriteError> {
    if account_id <= 0
        || generation <= 0
        || input.size <= 0
        || input.size > 1_073_741_824
        || input.mime.is_empty()
        || input.mime.len() > 255
        || !input.mime.bytes().all(|b| b.is_ascii_graphic())
    {
        return Err(WriteError::Validation("invalid raw upload identity"));
    }
    let format = media_format(input.mime).ok_or(WriteError::Validation("unsupported raw MIME"))?;
    if usize::try_from(input.size).map_or(true, |size| size >= format.input_size_limit) {
        return Err(WriteError::Validation("raw upload is too large"));
    }
    let media_id = sqlx::query_scalar(
        "INSERT INTO media_attachments (account_id, type, processing, remote_url,
           file_meta, created_at, updated_at)
         SELECT id, 0, 0, '', '{}'::json, clock_timestamp(), clock_timestamp()
         FROM accounts WHERE id = $1 AND domain IS NULL
         RETURNING id",
    )
    .bind(account_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(WriteError::NotFound)?;
    let identity = UploadIdentity {
        media_id,
        account_id,
        generation,
    };
    sqlx::query(
        "INSERT INTO rustodon.local_uploads
           (media_id, account_id, generation, raw_mime, raw_size, raw_sha256, raw_path)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(media_id)
    .bind(account_id)
    .bind(generation)
    .bind(input.mime)
    .bind(input.size)
    .bind(input.sha256.as_slice())
    .bind(raw_path(identity))
    .execute(&mut **tx)
    .await?;
    Ok(identity)
}

#[must_use]
pub fn raw_path(identity: UploadIdentity) -> String {
    format!(
        "local_uploads/{}/{}/input",
        identity.media_id, identity.generation
    )
}

/// Load the retained cleanup manifest, including after public-row deletion.
/// # Errors
/// Returns database failures.
pub async fn load_in(
    tx: &mut Transaction<'_, Postgres>,
    identity: UploadIdentity,
) -> Result<Option<UploadState>, WriteError> {
    Ok(sqlx::query_as(
        "SELECT media_id, account_id, generation, accepted, claim, raw_mime,
                raw_size, raw_sha256, raw_path, output_paths
         FROM rustodon.local_uploads WHERE media_id = $1 AND account_id = $2
           AND generation = $3 FOR UPDATE",
    )
    .bind(identity.media_id)
    .bind(identity.account_id)
    .bind(identity.generation)
    .fetch_optional(&mut **tx)
    .await?)
}

async fn lock_pending(
    tx: &mut Transaction<'_, Postgres>,
    identity: UploadIdentity,
) -> Result<(), WriteError> {
    let exists = sqlx::query_scalar::<_, i64>(
        "SELECT id FROM media_attachments WHERE id = $1 AND account_id = $2
           AND status_id IS NULL AND remote_url = '' AND file_file_name IS NULL
           AND processing IN (0, 1) FOR UPDATE",
    )
    .bind(identity.media_id)
    .bind(identity.account_id)
    .fetch_optional(&mut **tx)
    .await?;
    exists.ok_or(WriteError::NotFound).map(|_| ())
}

/// Mark durable acceptance and record its processing intent in the SAME transaction.
/// The next integration must supply a registered, generation-keyed processing spec.
/// There are intentionally no live callers or new job kinds in this slice. Duplicate
/// acceptance does not reset an already-dispatched event.
/// # Errors
/// Rejects missing/stale/deleted ownership or an invalid outbox spec.
pub async fn accept_in(
    tx: &mut Transaction<'_, Postgres>,
    identity: UploadIdentity,
    job: &JobSpec,
) -> Result<(), WriteError> {
    let key = format!(
        "mastodon:media:{}:upload:{}",
        identity.media_id, identity.generation
    );
    if job.logical_key_value() != Some(key.as_str())
        || job.arguments().get("media_id").and_then(Value::as_i64) != Some(identity.media_id)
        || job.arguments().get("account_id").and_then(Value::as_i64) != Some(identity.account_id)
        || job.arguments().get("generation").and_then(Value::as_i64) != Some(identity.generation)
    {
        return Err(WriteError::Validation(
            "upload processing intent identity mismatch",
        ));
    }
    lock_pending(tx, identity).await?;
    let state = load_in(tx, identity).await?.ok_or(WriteError::Conflict)?;
    if !state.accepted {
        if !record_outbox_once_in(tx, job).await? {
            return Err(WriteError::Conflict);
        }
        sqlx::query("UPDATE rustodon.local_uploads SET accepted = true WHERE media_id = $1")
            .bind(identity.media_id)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

/// Fence an older processor attempt without losing its cleanup manifest.
/// # Errors
/// Rejects unaccepted, stale, deleted, attached, or ready work.
pub async fn claim_in(
    tx: &mut Transaction<'_, Postgres>,
    identity: UploadIdentity,
) -> Result<UploadClaim, WriteError> {
    lock_pending(tx, identity).await?;
    let claim = sqlx::query_scalar(
        "UPDATE rustodon.local_uploads SET claim = claim + 1
         WHERE media_id = $1 AND account_id = $2 AND generation = $3 AND accepted
         RETURNING claim",
    )
    .bind(identity.media_id)
    .bind(identity.account_id)
    .bind(identity.generation)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(WriteError::Conflict)?;
    sqlx::query("UPDATE media_attachments SET processing = 1 WHERE id = $1")
        .bind(identity.media_id)
        .execute(&mut **tx)
        .await?;
    Ok(UploadClaim { identity, claim })
}

async fn owned_pending(
    tx: &mut Transaction<'_, Postgres>,
    token: UploadClaim,
) -> Result<UploadState, WriteError> {
    lock_pending(tx, token.identity).await?;
    let state = load_in(tx, token.identity)
        .await?
        .ok_or(WriteError::Conflict)?;
    if !state.accepted || state.claim != token.claim || token.claim <= 0 {
        return Err(WriteError::Conflict);
    }
    Ok(state)
}

/// Register exact output files before writing. A generation's manifest is immutable:
/// retries overwrite only these same files under the account lock, never invent paths.
/// # Errors
/// Rejects stale claims and foreign, unsafe, duplicate, or changed manifests.
pub async fn register_outputs_in(
    tx: &mut Transaction<'_, Postgres>,
    token: UploadClaim,
    output: &MediaAttachmentCreate,
) -> Result<(), WriteError> {
    let paths = output_paths(token.identity, output)?;
    let state = owned_pending(tx, token).await?;
    if !state.output_paths.is_empty() && state.output_paths != paths {
        return Err(WriteError::Conflict);
    }
    sqlx::query("UPDATE rustodon.local_uploads SET output_paths = $2 WHERE media_id = $1")
        .bind(token.identity.media_id)
        .bind(&paths)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

fn output_paths(
    identity: UploadIdentity,
    output: &MediaAttachmentCreate,
) -> Result<Vec<String>, WriteError> {
    validate_media_attachment_create(output)?;
    if !output.file_meta.is_object() {
        return Err(WriteError::Validation("invalid processed metadata"));
    }
    let metadata = PaperclipMetadata {
        attachment: PaperclipAttachment::MediaFile,
        id: identity.media_id,
        remote: false,
        storage_schema_version: Some(1),
        file_name: output.file_name.clone(),
        content_type: Some(output.content_type.clone()),
        variant: None,
    };
    let mut paths = ["original", "small"]
        .into_iter()
        .filter_map(|style| metadata.relative_path(style))
        .collect::<Vec<_>>();
    let count = if media_format(&output.content_type)
        .is_some_and(|format| format.preview_content_type.is_some())
    {
        2
    } else {
        1
    };
    if paths.len() != count {
        return Err(WriteError::Validation("invalid upload output manifest"));
    }
    paths.sort();
    Ok(paths)
}

/// Publish only a previously committed exact manifest. Preserve the row's latest
/// description and focus rather than replaying the processor's stale input metadata.
/// The caller must verify/write/fsync artifacts under the account lock before this
/// transaction. Ownership remains until raw cleanup; ready output ownership is public.
/// # Errors
/// Rejects stale/deleted work, invalid metadata, or an unregistered output manifest.
pub async fn publish_in(
    tx: &mut Transaction<'_, Postgres>,
    token: UploadClaim,
    output: &MediaAttachmentCreate,
) -> Result<(), WriteError> {
    let paths = output_paths(token.identity, output)?;
    let current = sqlx::query_as::<_, (Option<i32>, Option<String>, Option<Value>)>(
        "SELECT processing, file_file_name, file_meta FROM media_attachments
         WHERE id = $1 AND account_id = $2 AND remote_url = '' FOR UPDATE",
    )
    .bind(token.identity.media_id)
    .bind(token.identity.account_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(WriteError::NotFound)?;
    let state = load_in(tx, token.identity)
        .await?
        .ok_or(WriteError::Conflict)?;
    if !state.accepted
        || state.claim != token.claim
        || token.claim <= 0
        || state.output_paths != paths
    {
        return Err(WriteError::Conflict);
    }
    if current.0 == Some(2) && current.1.as_deref() == Some(output.file_name.as_str()) {
        return Ok(());
    }
    lock_pending(tx, token.identity).await?;
    let meta = preserve_focus(
        output.file_meta.clone(),
        &current.2.unwrap_or_else(|| json!({})),
    );
    sqlx::query(
        "UPDATE media_attachments SET processing = 2, type = $2, file_file_name = $3,
           file_content_type = $4, file_file_size = $5, file_meta = $6::json,
           blurhash = $7, file_storage_schema_version = 1,
           file_updated_at = clock_timestamp(), updated_at = clock_timestamp() WHERE id = $1",
    )
    .bind(token.identity.media_id)
    .bind(output.media_type)
    .bind(&output.file_name)
    .bind(&output.content_type)
    .bind(output.file_size)
    .bind(meta)
    .bind(&output.blurhash)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Cancel interrupted staging without losing its pre-write raw manifest.
/// # Errors
/// Rejects accepted, stale, deleted, or already-published ownership.
pub async fn discard_staging_in(
    tx: &mut Transaction<'_, Postgres>,
    identity: UploadIdentity,
) -> Result<(), WriteError> {
    lock_pending(tx, identity).await?;
    let state = load_in(tx, identity).await?.ok_or(WriteError::Conflict)?;
    if state.accepted {
        return Err(WriteError::Conflict);
    }
    sqlx::query("DELETE FROM media_attachments WHERE id = $1 AND account_id = $2")
        .bind(identity.media_id)
        .bind(identity.account_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Fence terminal failure/deletion while retaining every file for later cleanup.
/// Cleanup may forget ownership only after unlink + directory durability succeeds.
/// # Errors
/// Rejects stale claims or a no-longer-pending media row.
pub async fn abandon_in(
    tx: &mut Transaction<'_, Postgres>,
    token: UploadClaim,
) -> Result<(), WriteError> {
    owned_pending(tx, token).await?;
    sqlx::query("DELETE FROM media_attachments WHERE id = $1 AND account_id = $2")
        .bind(token.identity.media_id)
        .bind(token.identity.account_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Forget an orphan only after the caller has durably removed its exact manifest.
/// Never discards ownership of a live pending upload (including interrupted staging).
/// # Errors
/// Rejects stale ownership or any surviving public media row.
pub async fn forget_orphan_in(
    tx: &mut Transaction<'_, Postgres>,
    identity: UploadIdentity,
) -> Result<(), WriteError> {
    let deleted = sqlx::query(
        "DELETE FROM rustodon.local_uploads WHERE media_id = $1 AND account_id = $2
           AND generation = $3 AND NOT EXISTS (SELECT 1 FROM media_attachments WHERE id = $1)",
    )
    .bind(identity.media_id)
    .bind(identity.account_id)
    .bind(identity.generation)
    .execute(&mut **tx)
    .await?;
    if deleted.rows_affected() != 1 {
        return Err(WriteError::Conflict);
    }
    Ok(())
}

// Publication uses processor-derived manifest/metadata rather than the raw MIME.
// Keep the merge independently testable: caller-provided description is never written.
#[must_use]
fn preserve_focus(mut processed: Value, current: &Value) -> Value {
    if !processed.is_object() {
        processed = json!({});
    }
    let object = processed.as_object_mut().expect("object ensured above");
    object.remove("focus");
    if let Some(focus) = current.get("focus") {
        object.insert("focus".into(), focus.clone());
    }
    processed
}

/// Retire raw-input ownership after durable raw unlink. Published files remain owned by
/// the public row. Caller must hold the account lock through unlink and this transaction.
/// # Errors
/// Rejects missing, stale, pending, deleted, or mismatched public ownership.
pub async fn retire_ready_in(
    tx: &mut Transaction<'_, Postgres>,
    identity: UploadIdentity,
) -> Result<(), WriteError> {
    let state = load_in(tx, identity).await?.ok_or(WriteError::Conflict)?;
    let row = sqlx::query_as::<_, (String, String)>(
        "SELECT file_file_name, file_content_type FROM media_attachments
         WHERE id = $1 AND account_id = $2 AND remote_url = '' AND processing = 2
           AND file_file_name IS NOT NULL AND file_storage_schema_version = 1 FOR UPDATE",
    )
    .bind(identity.media_id)
    .bind(identity.account_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(WriteError::Conflict)?;
    let metadata = PaperclipMetadata {
        attachment: PaperclipAttachment::MediaFile,
        id: identity.media_id,
        remote: false,
        storage_schema_version: Some(1),
        file_name: row.0,
        content_type: Some(row.1),
        variant: None,
    };
    let mut paths = ["original", "small"]
        .into_iter()
        .filter_map(|style| metadata.relative_path(style))
        .collect::<Vec<_>>();
    paths.sort();
    if !state.accepted || state.claim <= 0 || paths.is_empty() || paths != state.output_paths {
        return Err(WriteError::Conflict);
    }
    sqlx::query("DELETE FROM rustodon.local_uploads WHERE media_id = $1 AND account_id = $2 AND generation = $3")
        .bind(identity.media_id).bind(identity.account_id).bind(identity.generation)
        .execute(&mut **tx).await?;
    Ok(())
}
