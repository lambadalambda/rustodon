//! Shared remote actor image state. Account timestamps are second-precision in Mastodon;
//! advance them monotonically so removal/re-add cannot revive an earlier download.
use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use sqlx::{Postgres, Transaction};

use super::WriteError;
use crate::jobs::{
    ACTIVITYPUB_PROFILE_MEDIA_CLEANUP_JOB_KIND, ACTIVITYPUB_PROFILE_MEDIA_FETCH_JOB_KIND, JobSpec,
    Lane, record_outbox_in,
};
use crate::paperclip::{PaperclipAttachment, PaperclipMetadata};

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ProfileImageSlot {
    Avatar,
    Header,
}

impl ProfileImageSlot {
    pub(crate) const fn column(self) -> &'static str {
        match self {
            Self::Avatar => "avatar",
            Self::Header => "header",
        }
    }
    pub(crate) const fn attachment(self) -> PaperclipAttachment {
        match self {
            Self::Avatar => PaperclipAttachment::AccountAvatar,
            Self::Header => PaperclipAttachment::AccountHeader,
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct ProfileImageJob {
    pub account_id: i64,
    pub actor_uri: String,
    pub domain: String,
    pub slot: ProfileImageSlot,
    pub remote_url: String,
    pub version: i64,
}

#[derive(sqlx::FromRow)]
pub(crate) struct ProfileImageState {
    pub domain: Option<String>,
    pub uri: String,
    pub suspended_at: Option<NaiveDateTime>,
    pub remote_url: Option<String>,
    pub file_name: Option<String>,
    pub content_type: Option<String>,
    pub file_size: Option<i32>,
    pub storage_schema_version: Option<i32>,
    pub version: Option<i64>,
}

impl ProfileImageState {
    pub(crate) fn matches(&self, job: &ProfileImageJob) -> bool {
        self.domain.as_deref() == Some(&job.domain)
            && self.uri == job.actor_uri
            && self.suspended_at.is_none()
            && self.remote_url.as_deref() == Some(&job.remote_url)
            && self.version == Some(job.version)
    }
    pub(crate) fn metadata(&self, id: i64, slot: ProfileImageSlot) -> Option<PaperclipMetadata> {
        Some(PaperclipMetadata {
            attachment: slot.attachment(),
            id,
            remote: self.domain.is_some(),
            storage_schema_version: self.storage_schema_version,
            file_name: self.file_name.clone()?,
            content_type: self.content_type.clone(),
            variant: None,
        })
    }
}

pub(crate) async fn image_state(
    transaction: &mut Transaction<'_, Postgres>,
    id: i64,
    slot: ProfileImageSlot,
) -> Result<Option<ProfileImageState>, sqlx::Error> {
    let column = slot.column();
    sqlx::query_as(&format!(
        "SELECT domain, uri, suspended_at, {column}_remote_url AS remote_url,
         {column}_file_name AS file_name, {column}_content_type AS content_type,
         {column}_file_size AS file_size, {column}_storage_schema_version AS storage_schema_version,
         extract(epoch FROM {column}_updated_at)::bigint AS version
         FROM accounts WHERE id = $1 FOR UPDATE"
    ))
    .bind(id)
    .fetch_optional(&mut **transaction)
    .await
}

pub(crate) fn image_paths(metadata: &PaperclipMetadata) -> Vec<String> {
    ["original", "static"]
        .into_iter()
        .filter_map(|style| metadata.relative_path(style))
        .collect()
}

pub(crate) fn cleanup_job(id: i64, slot: ProfileImageSlot, paths: &[String]) -> JobSpec {
    JobSpec::new(
        Lane::Maintenance,
        ACTIVITYPUB_PROFILE_MEDIA_CLEANUP_JOB_KIND,
        serde_json::json!({"account_id": id, "slot": slot, "paths": paths}),
    )
}

/// Called in the actor write transaction for both discovery and verified Update.
/// Missing fields retain their URL; discovery/refresh gets a bounded cache check,
/// allowing refresh to repair missing files as well as missing SQL metadata.
pub(crate) async fn persist_images(
    transaction: &mut Transaction<'_, Postgres>,
    id: i64,
    avatar: Option<&Option<String>>,
    header: Option<&Option<String>>,
    check_cache: bool,
) -> Result<(), WriteError> {
    for (slot, field) in [
        (ProfileImageSlot::Avatar, avatar),
        (ProfileImageSlot::Header, header),
    ] {
        let Some(state) = image_state(transaction, id, slot).await? else {
            continue;
        };
        let Some(domain) = state.domain.as_ref() else {
            continue;
        };
        let url = field
            .unwrap_or(&state.remote_url)
            .as_deref()
            .filter(|url| !url.is_empty());
        let changed = url != state.remote_url.as_deref().filter(|url| !url.is_empty());
        if url.is_none() && !changed && state.file_name.is_none() {
            continue;
        }
        let complete = state.file_name.is_some()
            && state.content_type.is_some()
            && state.file_size.is_some_and(|size| size > 0);
        if !changed && complete && !check_cache {
            continue;
        }
        let clear = changed || url.is_none();
        if clear && let Some(metadata) = state.metadata(id, slot) {
            let paths = image_paths(&metadata);
            if !paths.is_empty() {
                record_outbox_in(transaction, &cleanup_job(id, slot, &paths)).await?;
            }
        }
        let column = slot.column();
        // header_remote_url is NOT NULL in the pinned schema.
        let stored_url = if matches!(slot, ProfileImageSlot::Header) {
            Some(url.unwrap_or(""))
        } else {
            url
        };
        let version: i64 = if !clear && let Some(version) = state.version {
            version
        } else {
            sqlx::query_scalar(&format!(
            "UPDATE accounts SET {column}_remote_url = $2,
             {column}_file_name = CASE WHEN $3 THEN NULL ELSE {column}_file_name END,
             {column}_content_type = CASE WHEN $3 THEN NULL ELSE {column}_content_type END,
             {column}_file_size = CASE WHEN $3 THEN NULL ELSE {column}_file_size END,
             {column}_storage_schema_version = CASE WHEN $3 THEN NULL ELSE {column}_storage_schema_version END,
             {column}_updated_at = greatest(date_trunc('second', clock_timestamp()),
                 {column}_updated_at + interval '1 second'), updated_at = clock_timestamp()
             WHERE id = $1 RETURNING extract(epoch FROM {column}_updated_at)::bigint"
        )).bind(id).bind(stored_url).bind(clear).fetch_one(&mut **transaction).await?
        };
        if let Some(url) = url
            && state.suspended_at.is_none()
        {
            let job = ProfileImageJob {
                account_id: id,
                actor_uri: state.uri.clone(),
                domain: domain.clone(),
                slot,
                remote_url: url.to_owned(),
                version,
            };
            record_outbox_in(
                transaction,
                &JobSpec::new(
                    Lane::Pull,
                    ACTIVITYPUB_PROFILE_MEDIA_FETCH_JOB_KIND,
                    serde_json::json!(job),
                )
                .max_attempts(4),
            )
            .await?;
        }
    }
    Ok(())
}
