//! Account and domain purge helpers and moderation job payloads.

#[allow(clippy::wildcard_imports)] // shares the parent module namespace
use super::*;

pub(super) fn account_update_job(account_id: i64, updated_at: NaiveDateTime) -> JobSpec {
    let updated_at_micros = updated_at.and_utc().timestamp_micros();
    JobSpec::new(
        Lane::Push,
        ACTIVITYPUB_ACCOUNT_UPDATE_JOB_KIND,
        json!({
            "account_id": account_id,
            "updated_at_micros": updated_at_micros
        }),
    )
    .logical_key(format!(
        "activitypub:account:{account_id}:update:{updated_at_micros}"
    ))
}

pub(super) fn account_delete_job(account_id: i64, actor_uri: &str) -> JobSpec {
    JobSpec::new(
        Lane::Push,
        ACTIVITYPUB_ACCOUNT_DELETE_JOB_KIND,
        json!({
            "account_id": account_id,
            "actor_uri": actor_uri,
        }),
    )
    .logical_key(format!("activitypub:account:{account_id}:delete"))
}

pub(super) fn account_purge_job(
    account_id: i64,
    deletion_request_id: i64,
    created_at: NaiveDateTime,
    origin: Option<&str>,
) -> JobSpec {
    let run_at = created_at.and_utc() + ChronoDuration::days(ACCOUNT_DELETION_DELAY_DAYS);
    let mut arguments = json!({
        "account_id": account_id,
        "deletion_request_id": deletion_request_id,
        "deletion_created_at_micros": created_at.and_utc().timestamp_micros(),
    });
    if let Some(origin) = origin {
        arguments["origin"] = json!(origin);
    }
    JobSpec::new(
        Lane::Maintenance,
        MASTODON_ACCOUNT_PURGE_JOB_KIND,
        arguments,
    )
    .run_at(run_at)
    .logical_key(format!("mastodon:account:{account_id}:purge"))
}

pub(super) fn domain_block_job(
    domain_block_id: i64,
    severance_event_id: Option<i64>,
    updated_at: NaiveDateTime,
    origin: &str,
) -> JobSpec {
    let updated_at_micros = updated_at.and_utc().timestamp_micros();
    JobSpec::new(
        Lane::Maintenance,
        MASTODON_DOMAIN_BLOCK_JOB_KIND,
        json!({
            "domain_block_id": domain_block_id,
            "severance_event_id": severance_event_id,
            "origin": origin,
        }),
    )
    .logical_key(format!(
        "mastodon:domain-block:{domain_block_id}:{updated_at_micros}"
    ))
}

pub(super) fn domain_purge_job(domain: &str) -> JobSpec {
    JobSpec::new(
        Lane::Maintenance,
        MASTODON_DOMAIN_PURGE_JOB_KIND,
        json!({"domain": domain}),
    )
    .logical_key(format!("mastodon:domain-purge:{domain}"))
}

pub(super) async fn protected_status_ids(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
) -> Result<Vec<i64>, WriteError> {
    let status_ids = sqlx::query_scalar::<_, String>(
        "SELECT status_id FROM (
           SELECT unnest(report.status_ids)::text AS status_id
             FROM reports report
            WHERE report.target_account_id = $1 AND report.action_taken_at IS NULL
           UNION
           SELECT warning_status_id
             FROM account_warnings warning
             CROSS JOIN LATERAL unnest(
                 COALESCE(warning.status_ids, ARRAY[]::varchar[])
             ) AS warning_status(warning_status_id)
            WHERE warning.target_account_id = $1 AND warning.overruled_at IS NULL
         ) protected
        WHERE status_id ~ '^[0-9]+$'
        ORDER BY status_id",
    )
    .bind(account_id)
    .fetch_all(&mut **transaction)
    .await?;
    Ok(status_ids
        .into_iter()
        .filter_map(|status_id| status_id.parse::<i64>().ok())
        .collect())
}

pub(super) async fn purge_remote_account(
    transaction: &mut Transaction<'_, Postgres>,
    pending_stream_events: &mut Vec<PendingStreamEvent>,
    account_id: i64,
) -> Result<(), WriteError> {
    let is_remote = sqlx::query_scalar::<_, bool>(
        "SELECT domain IS NOT NULL FROM accounts WHERE id = $1 FOR UPDATE",
    )
    .bind(account_id)
    .fetch_optional(&mut **transaction)
    .await?
    .unwrap_or(false);
    if !is_remote {
        return Ok(());
    }
    cancel_pending_account_job(transaction, ACTIVITYPUB_ACCOUNT_UPDATE_JOB_KIND, account_id)
        .await?;
    cancel_pending_account_job(transaction, ACTIVITYPUB_ACCOUNT_DELETE_JOB_KIND, account_id)
        .await?;
    purge_account_statuses(transaction, pending_stream_events, account_id, &[], true).await?;
    purge_account_mentions(transaction, account_id, &[]).await?;
    purge_account_media(transaction, account_id, &[]).await?;
    purge_account_relationships(transaction, account_id).await?;
    purge_account_notifications(transaction, account_id).await?;
    purge_remote_account_activity_notifications(transaction, account_id).await?;
    purge_account_associations(transaction, account_id).await?;
    purge_remote_account_non_cascading_associations(transaction, account_id).await?;
    sqlx::query("DELETE FROM account_stats WHERE account_id = $1")
        .bind(account_id)
        .execute(&mut **transaction)
        .await?;
    sqlx::query("DELETE FROM accounts WHERE id = $1 AND domain IS NOT NULL")
        .bind(account_id)
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

pub(super) async fn purge_account_user(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
) -> Result<(), WriteError> {
    sqlx::query(
        "UPDATE users SET disabled = true, updated_at = clock_timestamp()
          WHERE account_id = $1",
    )
    .bind(account_id)
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        "DELETE FROM invites
          WHERE user_id IN (SELECT id FROM users WHERE account_id = $1)
            AND uses = 0",
    )
    .bind(account_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

pub(super) async fn purge_account_profile(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
) -> Result<(), WriteError> {
    sqlx::query(
        "UPDATE accounts SET silenced_at = NULL,
            suspended_at = COALESCE(suspended_at, clock_timestamp()),
            suspension_origin = 0, locked = false, memorial = false,
            discoverable = false, trendable = false, display_name = '', note = '',
            fields = '[]'::jsonb, also_known_as = ARRAY[]::varchar[],
            moved_to_account_id = NULL, reviewed_at = NULL, requested_review_at = NULL,
            avatar_content_type = NULL, avatar_description = '', avatar_file_name = NULL,
            avatar_file_size = NULL, avatar_remote_url = NULL,
            avatar_storage_schema_version = NULL, avatar_updated_at = NULL,
            header_content_type = NULL, header_description = '', header_file_name = NULL,
            header_file_size = NULL, header_remote_url = '',
            header_storage_schema_version = NULL, header_updated_at = NULL,
            updated_at = clock_timestamp()
          WHERE id = $1",
    )
    .bind(account_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

#[allow(clippy::too_many_lines)]
pub(super) async fn account_media_metadata_for_cleanup(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
    remote: bool,
    protected_status_ids: &[i64],
) -> Result<Vec<PaperclipMetadata>, WriteError> {
    let Some((
        avatar_storage_schema_version,
        avatar_file_name,
        avatar_content_type,
        header_storage_schema_version,
        header_file_name,
        header_content_type,
    )) = sqlx::query_as::<
        _,
        (
            Option<i32>,
            Option<String>,
            Option<String>,
            Option<i32>,
            Option<String>,
            Option<String>,
        ),
    >(
        "SELECT avatar_storage_schema_version, avatar_file_name, avatar_content_type,
                header_storage_schema_version, header_file_name, header_content_type
           FROM accounts WHERE id = $1",
    )
    .bind(account_id)
    .fetch_optional(&mut **transaction)
    .await?
    else {
        return Ok(Vec::new());
    };
    let mut metadata = Vec::new();
    if let Some(file_name) = avatar_file_name.filter(|name| !name.is_empty()) {
        metadata.push(PaperclipMetadata {
            attachment: PaperclipAttachment::AccountAvatar,
            id: account_id,
            remote,
            storage_schema_version: avatar_storage_schema_version,
            file_name,
            content_type: avatar_content_type,
            variant: None,
        });
    }
    if let Some(file_name) = header_file_name.filter(|name| !name.is_empty()) {
        metadata.push(PaperclipMetadata {
            attachment: PaperclipAttachment::AccountHeader,
            id: account_id,
            remote,
            storage_schema_version: header_storage_schema_version,
            file_name,
            content_type: header_content_type,
            variant: None,
        });
    }
    for (
        media_id,
        file_storage_schema_version,
        file_file_name,
        file_content_type,
        remote_url,
        thumbnail_storage_schema_version,
        thumbnail_file_name,
        thumbnail_content_type,
        thumbnail_remote_url,
    ) in sqlx::query_as::<
        _,
        (
            i64,
            Option<i32>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<i32>,
            Option<String>,
            Option<String>,
            Option<String>,
        ),
    >(
        "SELECT id, file_storage_schema_version, file_file_name, file_content_type,
                remote_url, thumbnail_storage_schema_version, thumbnail_file_name,
                thumbnail_content_type, thumbnail_remote_url
           FROM media_attachments
          WHERE account_id = $1
            AND (status_id IS NULL OR status_id <> ALL($2::bigint[]))
          ORDER BY id",
    )
    .bind(account_id)
    .bind(protected_status_ids)
    .fetch_all(&mut **transaction)
    .await?
    {
        append_media_attachment_metadata(
            &mut metadata,
            media_id,
            file_storage_schema_version,
            file_file_name,
            file_content_type,
            remote_url,
            thumbnail_storage_schema_version,
            thumbnail_file_name,
            thumbnail_content_type,
            thumbnail_remote_url,
        );
    }
    Ok(metadata)
}

#[allow(clippy::needless_pass_by_value, clippy::too_many_arguments)]
pub(super) fn append_media_attachment_metadata(
    metadata: &mut Vec<PaperclipMetadata>,
    media_id: i64,
    file_storage_schema_version: Option<i32>,
    file_file_name: Option<String>,
    file_content_type: Option<String>,
    remote_url: Option<String>,
    thumbnail_storage_schema_version: Option<i32>,
    thumbnail_file_name: Option<String>,
    thumbnail_content_type: Option<String>,
    _thumbnail_remote_url: Option<String>,
) {
    if let Some(file_name) = file_file_name.filter(|name| !name.is_empty()) {
        metadata.push(PaperclipMetadata {
            attachment: PaperclipAttachment::MediaFile,
            id: media_id,
            remote: !remote_url.as_deref().is_none_or(rails_blank),
            storage_schema_version: file_storage_schema_version,
            file_name,
            content_type: file_content_type,
            variant: None,
        });
    }
    if let Some(file_name) = thumbnail_file_name.filter(|name| !name.is_empty()) {
        metadata.push(PaperclipMetadata {
            attachment: PaperclipAttachment::MediaThumbnail,
            id: media_id,
            remote: !remote_url.as_deref().is_none_or(rails_blank),
            storage_schema_version: thumbnail_storage_schema_version,
            file_name,
            content_type: thumbnail_content_type,
            variant: None,
        });
    }
}

#[allow(clippy::too_many_lines)]
pub(super) async fn domain_media_metadata(
    pool: &PgPool,
    domain: &str,
    include_subdomains: bool,
) -> Result<Vec<PaperclipMetadata>, WriteError> {
    let account_query = if include_subdomains {
        "SELECT id, avatar_storage_schema_version, avatar_file_name, avatar_content_type,
                header_storage_schema_version, header_file_name, header_content_type
           FROM accounts
          WHERE domain IS NOT NULL
            AND (lower(trim(trailing '.' FROM (CASE WHEN left(domain, 1) = '['
                     THEN split_part(domain, ']', 1) || ']' ELSE split_part(domain, ':', 1) END))) = lower($1)
              OR lower(trim(trailing '.' FROM (CASE WHEN left(domain, 1) = '['
                     THEN split_part(domain, ']', 1) || ']' ELSE split_part(domain, ':', 1) END))) LIKE '%.' || lower($1))"
    } else {
        "SELECT id, avatar_storage_schema_version, avatar_file_name, avatar_content_type,
                header_storage_schema_version, header_file_name, header_content_type
           FROM accounts
          WHERE domain IS NOT NULL AND lower(domain) = lower($1)"
    };
    let mut metadata = Vec::new();
    for (
        account_id,
        avatar_storage_schema_version,
        avatar_file_name,
        avatar_content_type,
        header_storage_schema_version,
        header_file_name,
        header_content_type,
    ) in sqlx::query_as::<
        _,
        (
            i64,
            Option<i32>,
            Option<String>,
            Option<String>,
            Option<i32>,
            Option<String>,
            Option<String>,
        ),
    >(account_query)
    .bind(domain)
    .fetch_all(pool)
    .await?
    {
        if let Some(file_name) = avatar_file_name.filter(|name| !name.is_empty()) {
            metadata.push(PaperclipMetadata {
                attachment: PaperclipAttachment::AccountAvatar,
                id: account_id,
                remote: true,
                storage_schema_version: avatar_storage_schema_version,
                file_name,
                content_type: avatar_content_type,
                variant: None,
            });
        }
        if let Some(file_name) = header_file_name.filter(|name| !name.is_empty()) {
            metadata.push(PaperclipMetadata {
                attachment: PaperclipAttachment::AccountHeader,
                id: account_id,
                remote: true,
                storage_schema_version: header_storage_schema_version,
                file_name,
                content_type: header_content_type,
                variant: None,
            });
        }
    }

    let media_query = if include_subdomains {
        "SELECT media.id, media.file_storage_schema_version, media.file_file_name,
                media.file_content_type, media.thumbnail_storage_schema_version,
                media.thumbnail_file_name, media.thumbnail_content_type
           FROM media_attachments media
           JOIN accounts account ON account.id = media.account_id
          WHERE account.domain IS NOT NULL
            AND (lower(trim(trailing '.' FROM (CASE WHEN left(account.domain, 1) = '['
                     THEN split_part(account.domain, ']', 1) || ']' ELSE split_part(account.domain, ':', 1) END))) = lower($1)
              OR lower(trim(trailing '.' FROM (CASE WHEN left(account.domain, 1) = '['
                     THEN split_part(account.domain, ']', 1) || ']' ELSE split_part(account.domain, ':', 1) END))) LIKE '%.' || lower($1))"
    } else {
        "SELECT media.id, media.file_storage_schema_version, media.file_file_name,
                media.file_content_type, media.thumbnail_storage_schema_version,
                media.thumbnail_file_name, media.thumbnail_content_type
           FROM media_attachments media
           JOIN accounts account ON account.id = media.account_id
          WHERE account.domain IS NOT NULL AND lower(account.domain) = lower($1)"
    };
    for (
        media_id,
        file_storage_schema_version,
        file_file_name,
        file_content_type,
        thumbnail_storage_schema_version,
        thumbnail_file_name,
        thumbnail_content_type,
    ) in sqlx::query_as::<
        _,
        (
            i64,
            Option<i32>,
            Option<String>,
            Option<String>,
            Option<i32>,
            Option<String>,
            Option<String>,
        ),
    >(media_query)
    .bind(domain)
    .fetch_all(pool)
    .await?
    {
        if let Some(file_name) = file_file_name.filter(|name| !name.is_empty()) {
            metadata.push(PaperclipMetadata {
                attachment: PaperclipAttachment::MediaFile,
                id: media_id,
                remote: true,
                storage_schema_version: file_storage_schema_version,
                file_name,
                content_type: file_content_type,
                variant: None,
            });
        }
        if let Some(file_name) = thumbnail_file_name.filter(|name| !name.is_empty()) {
            metadata.push(PaperclipMetadata {
                attachment: PaperclipAttachment::MediaThumbnail,
                id: media_id,
                remote: true,
                storage_schema_version: thumbnail_storage_schema_version,
                file_name,
                content_type: thumbnail_content_type,
                variant: None,
            });
        }
    }

    let emoji_query = if include_subdomains {
        "SELECT id, image_storage_schema_version, image_file_name, image_content_type
           FROM custom_emojis
          WHERE domain IS NOT NULL
            AND (lower(trim(trailing '.' FROM domain)) = lower($1)
              OR lower(trim(trailing '.' FROM domain)) LIKE '%.' || lower($1))"
    } else {
        "SELECT id, image_storage_schema_version, image_file_name, image_content_type
           FROM custom_emojis
          WHERE domain IS NOT NULL AND lower(domain) = lower($1)"
    };
    for (emoji_id, storage_schema_version, file_name, content_type) in
        sqlx::query_as::<_, (i64, Option<i32>, Option<String>, Option<String>)>(emoji_query)
            .bind(domain)
            .fetch_all(pool)
            .await?
    {
        if let Some(file_name) = file_name.filter(|name| !name.is_empty()) {
            metadata.push(PaperclipMetadata {
                attachment: PaperclipAttachment::CustomEmojiImage,
                id: emoji_id,
                remote: true,
                storage_schema_version,
                file_name,
                content_type,
                variant: None,
            });
        }
    }
    Ok(metadata)
}

#[allow(clippy::too_many_lines)]
pub(super) async fn purge_account_statuses(
    transaction: &mut Transaction<'_, Postgres>,
    pending_stream_events: &mut Vec<PendingStreamEvent>,
    account_id: i64,
    protected_status_ids: &[i64],
    emit_stream_events: bool,
) -> Result<(), WriteError> {
    let statuses = sqlx::query_as::<_, (i64, i64, Option<i64>, Option<i64>, i32)>(
        "WITH RECURSIVE owned AS (
           SELECT id, account_id, reblog_of_id, in_reply_to_id, visibility
             FROM statuses
            WHERE account_id = $1 AND deleted_at IS NULL
              AND id <> ALL($2::bigint[])
         ), affected_ids(id) AS (
           SELECT id FROM owned
           UNION
           SELECT child.id
             FROM statuses child
             JOIN affected_ids parent ON parent.id = child.reblog_of_id
            WHERE child.deleted_at IS NULL
         )
         SELECT status_row.id, status_row.account_id, status_row.reblog_of_id,
                status_row.in_reply_to_id, status_row.visibility
           FROM statuses status_row
           JOIN affected_ids ON affected_ids.id = status_row.id
          ORDER BY status_row.id
          FOR UPDATE OF status_row",
    )
    .bind(account_id)
    .bind(protected_status_ids)
    .fetch_all(&mut **transaction)
    .await?;
    let status_ids = statuses
        .iter()
        .map(|(status_id, ..)| *status_id)
        .collect::<Vec<_>>();
    let affected_quotes = sqlx::query_as::<_, (i64, Option<String>)>(
        "SELECT id, activity_uri FROM quotes \
          WHERE status_id = ANY($1::bigint[]) OR quoted_status_id = ANY($1::bigint[]) \
          ORDER BY id FOR UPDATE",
    )
    .bind(&status_ids)
    .fetch_all(&mut **transaction)
    .await?;
    for (quote_id, request_uri) in affected_quotes {
        cancel_quote_request_outbox(transaction, quote_id, request_uri.as_deref()).await?;
    }
    let mut timeline_snapshots = if emit_stream_events {
        status_timeline_snapshots(transaction, &status_ids).await?
    } else {
        HashMap::new()
    };
    let accepted_quote_targets = sqlx::query_scalar::<_, i64>(
        "SELECT quoted_status_id FROM quotes
           WHERE status_id = ANY($1::bigint[])
             AND state = 1
             AND quoted_status_id IS NOT NULL
             AND NOT (quoted_status_id = ANY($2::bigint[]))",
    )
    .bind(&status_ids)
    .bind(&status_ids)
    .fetch_all(&mut **transaction)
    .await?;
    let mut accepted_quote_counts = HashMap::new();
    for quoted_status_id in accepted_quote_targets {
        accepted_quote_counts
            .entry(quoted_status_id)
            .and_modify(|count: &mut i64| *count = count.saturating_add(1))
            .or_insert(1_i64);
    }

    delete_remote_status_notifications(transaction, &status_ids).await?;
    remove_favourites_for_account_and_statuses(transaction, account_id, &status_ids).await?;
    remove_poll_data_for_account_and_statuses(
        transaction,
        account_id,
        &status_ids,
        protected_status_ids,
    )
    .await?;
    remove_statuses_from_account_conversations(transaction, &status_ids).await?;
    for status_id in &status_ids {
        cancel_status_outbox(transaction, *status_id).await?;
        cancel_quote_decision_outbox_for_target(transaction, *status_id).await?;
        if emit_stream_events && let Some(snapshot) = timeline_snapshots.remove(status_id) {
            collect_status_delete_stream_events_with_snapshot(
                transaction,
                pending_stream_events,
                *status_id,
                snapshot,
            )
            .await?;
        }
    }
    if !status_ids.is_empty() {
        sqlx::query("DELETE FROM media_attachments WHERE status_id = ANY($1::bigint[])")
            .bind(&status_ids)
            .execute(&mut **transaction)
            .await?;
        sqlx::query("DELETE FROM status_pins WHERE status_id = ANY($1::bigint[])")
            .bind(&status_ids)
            .execute(&mut **transaction)
            .await?;
        sqlx::query("DELETE FROM bookmarks WHERE status_id = ANY($1::bigint[])")
            .bind(&status_ids)
            .execute(&mut **transaction)
            .await?;
        sqlx::query("DELETE FROM statuses WHERE id = ANY($1::bigint[])")
            .bind(&status_ids)
            .execute(&mut **transaction)
            .await?;
    }
    let mut status_deltas = HashMap::new();
    for (_, status_account_id, reblog_of_id, in_reply_to_id, visibility) in &statuses {
        if *visibility != 3 {
            add_account_stats_delta(
                &mut status_deltas,
                *status_account_id,
                AccountStatsDelta {
                    statuses: -1,
                    ..AccountStatsDelta::default()
                },
            );
        }
        if let Some(reblog_of_id) = reblog_of_id {
            decrement_reblog_count(transaction, *reblog_of_id).await?;
        } else if *visibility < 2
            && let Some(in_reply_to_id) = in_reply_to_id
        {
            decrement_reply_count(transaction, *in_reply_to_id).await?;
        }
    }
    apply_account_stats_deltas(transaction, status_deltas).await?;
    for (quoted_status_id, quote_count) in accepted_quote_counts {
        sqlx::query(
            "UPDATE status_stats
                SET quotes_count = GREATEST(0, quotes_count - $2),
                    updated_at = clock_timestamp()
              WHERE status_id = $1",
        )
        .bind(quoted_status_id)
        .bind(quote_count)
        .execute(&mut **transaction)
        .await?;
    }
    Ok(())
}

pub(super) async fn purge_account_mentions(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
    protected_status_ids: &[i64],
) -> Result<(), WriteError> {
    sqlx::query(
        "DELETE FROM mentions
          WHERE account_id = $1 AND status_id <> ALL($2::bigint[])",
    )
    .bind(account_id)
    .bind(protected_status_ids)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

pub(super) async fn purge_account_media(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
    protected_status_ids: &[i64],
) -> Result<(), WriteError> {
    sqlx::query(
        "DELETE FROM media_attachments
          WHERE account_id = $1
            AND (status_id IS NULL OR status_id <> ALL($2::bigint[]))",
    )
    .bind(account_id)
    .bind(protected_status_ids)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

#[allow(clippy::too_many_lines)]
pub(super) async fn purge_account_relationships(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
) -> Result<(), WriteError> {
    let follows = sqlx::query_as::<_, (i64, i64, i64, Option<String>)>(
        "SELECT id, account_id, target_account_id, uri
           FROM follows
          WHERE account_id = $1 OR target_account_id = $1
          ORDER BY account_id, target_account_id
          FOR UPDATE",
    )
    .bind(account_id)
    .fetch_all(&mut **transaction)
    .await?;
    let follow_requests = sqlx::query_as::<_, (i64, i64, Option<String>)>(
        "SELECT id, target_account_id, uri
           FROM follow_requests
          WHERE account_id = $1 OR target_account_id = $1
          ORDER BY account_id, target_account_id
          FOR UPDATE",
    )
    .bind(account_id)
    .fetch_all(&mut **transaction)
    .await?;
    let blocks = sqlx::query_as::<_, (i64, Option<String>)>(
        "SELECT id, uri FROM blocks
          WHERE account_id = $1 OR target_account_id = $1
          ORDER BY id FOR UPDATE",
    )
    .bind(account_id)
    .fetch_all(&mut **transaction)
    .await?;
    let mutes = sqlx::query_scalar::<_, i64>(
        "SELECT id FROM mutes
          WHERE account_id = $1 OR target_account_id = $1
          ORDER BY id FOR UPDATE",
    )
    .bind(account_id)
    .fetch_all(&mut **transaction)
    .await?;
    for (_, _, _, uri) in &follows {
        if let Some(uri) = uri.as_deref().filter(|uri| !uri.is_empty()) {
            cancel_activitypub_delivery(transaction, uri).await?;
        }
    }
    for (_, _, uri) in &follow_requests {
        if let Some(uri) = uri.as_deref().filter(|uri| !uri.is_empty()) {
            cancel_activitypub_delivery(transaction, uri).await?;
        }
    }
    for (_, uri) in &blocks {
        if let Some(uri) = uri.as_deref().filter(|uri| !uri.is_empty()) {
            cancel_activitypub_delivery(transaction, uri).await?;
        }
    }
    for mute_id in &mutes {
        cancel_pending_mute_expiry_events(transaction, *mute_id).await?;
    }
    sqlx::query("DELETE FROM follows WHERE account_id = $1 OR target_account_id = $1")
        .bind(account_id)
        .execute(&mut **transaction)
        .await?;
    sqlx::query("DELETE FROM follow_requests WHERE account_id = $1 OR target_account_id = $1")
        .bind(account_id)
        .execute(&mut **transaction)
        .await?;
    let mut relationship_deltas = HashMap::new();
    for (_, source_account_id, target_account_id, _) in &follows {
        add_account_stats_delta(
            &mut relationship_deltas,
            *source_account_id,
            AccountStatsDelta {
                following: -1,
                ..AccountStatsDelta::default()
            },
        );
        add_account_stats_delta(
            &mut relationship_deltas,
            *target_account_id,
            AccountStatsDelta {
                followers: -1,
                ..AccountStatsDelta::default()
            },
        );
    }
    apply_account_stats_deltas(transaction, relationship_deltas).await?;
    for (follow_id, _, target_account_id, _) in &follows {
        delete_activity_notifications(transaction, *target_account_id, *follow_id, "Follow")
            .await?;
    }
    for (request_id, target_account_id, _) in follow_requests {
        delete_activity_notifications(transaction, target_account_id, request_id, "FollowRequest")
            .await?;
    }
    sqlx::query("DELETE FROM blocks WHERE account_id = $1 OR target_account_id = $1")
        .bind(account_id)
        .execute(&mut **transaction)
        .await?;
    sqlx::query("DELETE FROM mutes WHERE account_id = $1 OR target_account_id = $1")
        .bind(account_id)
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

pub(super) async fn purge_account_notifications(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
) -> Result<(), WriteError> {
    let notification_keys = sqlx::query_as::<_, (i64, i64, String)>(
        "SELECT DISTINCT notification.account_id, notification.activity_id,
                notification.activity_type
           FROM notifications notification
          WHERE notification.account_id = $1 OR notification.from_account_id = $1
          ORDER BY notification.account_id, notification.activity_id,
                   notification.activity_type",
    )
    .bind(account_id)
    .fetch_all(&mut **transaction)
    .await?;
    for (recipient_account_id, activity_id, activity_type) in notification_keys {
        delete_activity_notifications(
            transaction,
            recipient_account_id,
            activity_id,
            &activity_type,
        )
        .await?;
    }
    sqlx::query("DELETE FROM notifications WHERE account_id = $1 OR from_account_id = $1")
        .bind(account_id)
        .execute(&mut **transaction)
        .await?;
    sqlx::query(
        "DELETE FROM notification_requests
          WHERE account_id = $1 OR from_account_id = $1",
    )
    .bind(account_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

pub(super) async fn purge_remote_account_activity_notifications(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
) -> Result<(), WriteError> {
    let notification_keys = sqlx::query_as::<_, (i64, i64, String)>(
        "WITH activities(activity_id, activity_type) AS (
             SELECT id, 'Account'::text FROM accounts WHERE id = $1
             UNION
             SELECT id, 'Status'::text FROM statuses WHERE account_id = $1
             UNION
             SELECT id, 'Mention'::text FROM mentions WHERE account_id = $1
             UNION
             SELECT id, 'Favourite'::text FROM favourites WHERE account_id = $1
             UNION
             SELECT id, 'Poll'::text FROM polls WHERE account_id = $1
             UNION
             SELECT id, 'Quote'::text FROM quotes WHERE account_id = $1
             UNION
             SELECT id, 'AccountRelationshipSeveranceEvent'::text
               FROM account_relationship_severance_events WHERE account_id = $1
             UNION
             SELECT id, 'AccountWarning'::text FROM account_warnings
              WHERE account_id = $1 OR target_account_id = $1
             UNION
             SELECT id, 'GeneratedAnnualReport'::text
               FROM generated_annual_reports WHERE account_id = $1
             UNION
             SELECT id, 'Report'::text FROM reports
              WHERE account_id = $1 OR target_account_id = $1
             UNION
             SELECT id, 'CollectionItem'::text FROM collection_items
              WHERE account_id = $1 OR collection_id IN (
                  SELECT id FROM collections WHERE account_id = $1)
             UNION
             SELECT id, 'Collection'::text FROM collections WHERE account_id = $1
         )
         SELECT DISTINCT notification.account_id, notification.activity_id,
                         notification.activity_type
           FROM notifications notification
           JOIN activities activity
             ON activity.activity_id = notification.activity_id
            AND activity.activity_type = notification.activity_type
          ORDER BY notification.account_id, notification.activity_type,
                   notification.activity_id",
    )
    .bind(account_id)
    .fetch_all(&mut **transaction)
    .await?;
    for (recipient_account_id, activity_id, activity_type) in notification_keys {
        delete_activity_notifications(
            transaction,
            recipient_account_id,
            activity_id,
            &activity_type,
        )
        .await?;
    }
    Ok(())
}

pub(super) async fn purge_remote_account_non_cascading_associations(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
) -> Result<(), WriteError> {
    for query in [
        "DELETE FROM generated_annual_reports WHERE account_id = $1",
        "DELETE FROM fasp_follow_recommendations
           WHERE requesting_account_id = $1 OR recommended_account_id = $1",
    ] {
        sqlx::query(query)
            .bind(account_id)
            .execute(&mut **transaction)
            .await?;
    }
    Ok(())
}

pub(super) async fn purge_account_associations(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
) -> Result<(), WriteError> {
    for query in [
        "DELETE FROM account_notes WHERE account_id = $1",
        "DELETE FROM account_pins WHERE account_id = $1",
        "DELETE FROM account_aliases WHERE account_id = $1",
        "DELETE FROM account_domain_blocks WHERE account_id = $1",
        "DELETE FROM account_migrations WHERE account_id = $1",
        "DELETE FROM featured_tags WHERE account_id = $1",
        "DELETE FROM bookmarks WHERE account_id = $1",
        "DELETE FROM report_notes WHERE account_id = $1",
        "DELETE FROM scheduled_statuses WHERE account_id = $1",
        "DELETE FROM status_pins WHERE account_id = $1",
        "DELETE FROM tag_follows WHERE account_id = $1",
        "DELETE FROM accounts_tags WHERE account_id = $1",
        "DELETE FROM account_conversations WHERE account_id = $1",
        "DELETE FROM conversation_mutes WHERE account_id = $1",
        "DELETE FROM custom_filters WHERE account_id = $1",
    ] {
        sqlx::query(query)
            .bind(account_id)
            .execute(&mut **transaction)
            .await?;
    }
    sqlx::query(
        "DELETE FROM collection_items
          WHERE account_id = $1
             OR collection_id IN (SELECT id FROM collections WHERE account_id = $1)",
    )
    .bind(account_id)
    .execute(&mut **transaction)
    .await?;
    sqlx::query("DELETE FROM collections WHERE account_id = $1")
        .bind(account_id)
        .execute(&mut **transaction)
        .await?;
    sqlx::query(
        "DELETE FROM list_accounts
          WHERE account_id = $1
             OR list_id IN (SELECT id FROM lists WHERE account_id = $1)",
    )
    .bind(account_id)
    .execute(&mut **transaction)
    .await?;
    sqlx::query("DELETE FROM lists WHERE account_id = $1")
        .bind(account_id)
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

pub(super) async fn reject_remote_account_follows(
    transaction: &mut Transaction<'_, Postgres>,
    remote_account_id: i64,
    origin: &str,
) -> Result<(), WriteError> {
    let follows = sqlx::query_as::<_, (i64, i64, Option<String>)>(
        "SELECT follow_row.id, follow_row.target_account_id, follow_row.uri \
         FROM follows follow_row \
         JOIN accounts target ON target.id = follow_row.target_account_id \
         WHERE follow_row.account_id = $1 AND target.domain IS NULL \
         ORDER BY follow_row.id FOR UPDATE OF follow_row",
    )
    .bind(remote_account_id)
    .fetch_all(&mut **transaction)
    .await?;
    let follow_ids = follows
        .iter()
        .map(|(follow_id, _, _)| *follow_id)
        .collect::<Vec<_>>();
    sqlx::query("DELETE FROM follows WHERE id = ANY($1)")
        .bind(&follow_ids)
        .execute(&mut **transaction)
        .await?;
    let mut relationship_deltas = HashMap::new();
    for (_, local_account_id, _) in &follows {
        add_account_stats_delta(
            &mut relationship_deltas,
            remote_account_id,
            AccountStatsDelta {
                following: -1,
                ..AccountStatsDelta::default()
            },
        );
        add_account_stats_delta(
            &mut relationship_deltas,
            *local_account_id,
            AccountStatsDelta {
                followers: -1,
                ..AccountStatsDelta::default()
            },
        );
    }
    apply_account_stats_deltas(transaction, relationship_deltas).await?;
    for (follow_id, local_account_id, follow_uri) in follows {
        delete_activity_notifications(transaction, local_account_id, follow_id, "Follow").await?;
        if let Some(follow_uri) = follow_uri
            && let Some(remote_delivery) = remote_relationship_delivery(
                transaction,
                local_account_id,
                remote_account_id,
                origin,
            )
            .await?
        {
            cancel_activitypub_delivery(
                transaction,
                &format!("{}#accepts/follows/{follow_id}", remote_delivery.source_uri),
            )
            .await?;
            record_remote_reject_delivery(
                transaction,
                local_account_id,
                &remote_delivery,
                follow_id,
                &follow_uri,
            )
            .await?;
        }
    }
    Ok(())
}

pub(super) async fn undo_remote_account_follows(
    transaction: &mut Transaction<'_, Postgres>,
    remote_account_id: i64,
    origin: &str,
) -> Result<(), WriteError> {
    let follows = sqlx::query_as::<_, (i64, i64, Option<String>)>(
        "SELECT follow_row.id, follow_row.account_id, follow_row.uri \
           FROM follows follow_row \
           JOIN accounts source ON source.id = follow_row.account_id \
          WHERE follow_row.target_account_id = $1 AND source.domain IS NULL \
          ORDER BY follow_row.id FOR UPDATE OF follow_row",
    )
    .bind(remote_account_id)
    .fetch_all(&mut **transaction)
    .await?;
    for (_, local_account_id, follow_uri) in follows {
        let Some(follow_uri) = follow_uri.as_deref().filter(|uri| !uri.is_empty()) else {
            continue;
        };
        if let Some(remote_delivery) =
            remote_relationship_delivery(transaction, local_account_id, remote_account_id, origin)
                .await?
        {
            cancel_activitypub_delivery(transaction, follow_uri).await?;
            record_remote_undo_follow_delivery(
                transaction,
                local_account_id,
                &remote_delivery,
                follow_uri,
                origin,
            )
            .await?;
        }
    }
    Ok(())
}

pub(super) async fn insert_instance_admin_action_log(
    transaction: &mut Transaction<'_, Postgres>,
    acting_account_id: i64,
    domain: &str,
) -> Result<(), WriteError> {
    sqlx::query(
        "INSERT INTO admin_action_logs ( \
             account_id, action, created_at, human_identifier, route_param, target_id, \
             target_type, updated_at) \
         VALUES ($1, 'destroy', clock_timestamp(), $2, $2, NULL, 'Instance', clock_timestamp())",
    )
    .bind(acting_account_id)
    .bind(domain)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

pub(super) async fn insert_admin_action_log(
    transaction: &mut Transaction<'_, Postgres>,
    acting_account_id: i64,
    action: &str,
    target_id: i64,
    target_type: &str,
    human_identifier: String,
    route_param: Option<&str>,
) -> Result<(), WriteError> {
    sqlx::query(
        "INSERT INTO admin_action_logs ( \
             account_id, action, created_at, human_identifier, route_param, target_id, \
             target_type, updated_at) \
         VALUES ($1, $2, clock_timestamp(), $3, $4, $5, $6, clock_timestamp())",
    )
    .bind(acting_account_id)
    .bind(action)
    .bind(human_identifier)
    .bind(route_param)
    .bind(target_id)
    .bind(target_type)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

pub(super) fn normalize_domain_block_domain(domain: &str) -> Result<String, WriteError> {
    let domain = domain.trim();
    let domain = domain.strip_suffix('/').unwrap_or(domain);
    if domain.is_empty() || domain.contains('/') {
        return Err(WriteError::InvalidInput("domain block domain is invalid"));
    }
    let domain = canonical_remote_domain(domain)
        .map_err(|_| WriteError::InvalidInput("domain block domain is invalid"))?;
    if domain.contains(':') {
        return Err(WriteError::InvalidInput(
            "domain block domain must not include a port",
        ));
    }
    if domain.len() >= 256
        || domain.split('.').any(|label| {
            label.is_empty()
                || label.len() > 63
                || !label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
    {
        return Err(WriteError::InvalidInput("domain block domain is invalid"));
    }
    Ok(domain)
}

#[allow(clippy::fn_params_excessive_bools)]
pub(super) fn domain_block_is_stricter(
    severity: i32,
    reject_media: bool,
    reject_reports: bool,
    existing_severity: Option<i32>,
    existing_reject_media: bool,
    existing_reject_reports: bool,
) -> bool {
    if severity == 1 {
        return true;
    }
    let Some(existing_severity) = existing_severity else {
        return false;
    };
    if !matches!(existing_severity, 0..=2) {
        return false;
    }
    if existing_severity == 1 && (severity == 0 || severity == 2) {
        return false;
    }
    if existing_severity == 0 && severity == 2 {
        return false;
    }
    (reject_media || !existing_reject_media) && (reject_reports || !existing_reject_reports)
}

pub(super) fn canonical_email_hash(email: &str) -> String {
    let email = email.to_ascii_lowercase();
    let mut parts = email.splitn(2, '@');
    let local = parts.next().unwrap_or_default();
    let domain = parts.next().unwrap_or_default();
    let local = local.split('+').next().unwrap_or_default().replace('.', "");
    format!(
        "{:x}",
        Sha256::digest(format!("{local}@{domain}").as_bytes())
    )
}

pub(super) async fn clear_domain_owned_account_restrictions(
    transaction: &mut Transaction<'_, Postgres>,
    domain: &str,
    created_at: NaiveDateTime,
) -> Result<(), WriteError> {
    let domain = domain_policy_hostname(domain);
    sqlx::query(
        "UPDATE accounts SET silenced_at = NULL \
         WHERE domain IS NOT NULL \
           AND (lower(trim(trailing '.' FROM (CASE WHEN left(domain, 1) = '[' \
                    THEN split_part(domain, ']', 1) || ']' ELSE split_part(domain, ':', 1) END))) = lower($1) \
             OR lower(trim(trailing '.' FROM (CASE WHEN left(domain, 1) = '[' \
                    THEN split_part(domain, ']', 1) || ']' ELSE split_part(domain, ':', 1) END))) LIKE '%.' || lower($1)) \
           AND silenced_at = $2",
    )
    .bind(&domain)
    .bind(created_at)
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        "UPDATE accounts SET suspended_at = NULL, suspension_origin = NULL \
         WHERE domain IS NOT NULL \
           AND (lower(trim(trailing '.' FROM (CASE WHEN left(domain, 1) = '[' \
                    THEN split_part(domain, ']', 1) || ']' ELSE split_part(domain, ':', 1) END))) = lower($1) \
             OR lower(trim(trailing '.' FROM (CASE WHEN left(domain, 1) = '[' \
                    THEN split_part(domain, ']', 1) || ']' ELSE split_part(domain, ':', 1) END))) LIKE '%.' || lower($1)) \
           AND suspended_at = $2",
    )
    .bind(&domain)
    .bind(created_at)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

pub(super) async fn apply_domain_account_restrictions(
    transaction: &mut Transaction<'_, Postgres>,
    domain: &str,
    severity: i32,
    created_at: NaiveDateTime,
) -> Result<(), WriteError> {
    let domain = domain_policy_hostname(domain);
    match severity {
        0 => {
            sqlx::query(
                "UPDATE accounts SET silenced_at = $2 \
                 WHERE domain IS NOT NULL \
                   AND (lower(trim(trailing '.' FROM (CASE WHEN left(domain, 1) = '[' \
                            THEN split_part(domain, ']', 1) || ']' ELSE split_part(domain, ':', 1) END))) = lower($1) \
                     OR lower(trim(trailing '.' FROM (CASE WHEN left(domain, 1) = '[' \
                            THEN split_part(domain, ']', 1) || ']' ELSE split_part(domain, ':', 1) END))) LIKE '%.' || lower($1)) \
                   AND silenced_at IS NULL",
            )
            .bind(&domain)
            .bind(created_at)
            .execute(&mut **transaction)
            .await?;
            Ok(())
        }
        1 => {
            sqlx::query(
                "UPDATE accounts SET silenced_at = NULL, suspended_at = $2, suspension_origin = 0 \
                  WHERE domain IS NOT NULL \
                    AND (lower(trim(trailing '.' FROM (CASE WHEN left(domain, 1) = '[' \
                             THEN split_part(domain, ']', 1) || ']' ELSE split_part(domain, ':', 1) END))) = lower($1) \
                      OR lower(trim(trailing '.' FROM (CASE WHEN left(domain, 1) = '[' \
                             THEN split_part(domain, ']', 1) || ']' ELSE split_part(domain, ':', 1) END))) LIKE '%.' || lower($1)) \
                    AND suspended_at IS NULL",
            )
            .bind(&domain)
            .bind(created_at)
            .execute(&mut **transaction)
            .await?;
            Ok(())
        }
        2 => Ok(()),
        _ => Err(WriteError::InvalidInput("domain block severity is invalid")),
    }
}

pub(super) async fn clear_domain_media(
    transaction: &mut Transaction<'_, Postgres>,
    domain: &str,
) -> Result<(), WriteError> {
    let domain = domain_policy_hostname(domain);
    sqlx::query(
        "UPDATE accounts SET
            avatar_file_name = NULL, avatar_content_type = NULL, avatar_file_size = NULL,
            avatar_updated_at = NULL, header_file_name = NULL, header_content_type = NULL,
            header_file_size = NULL, header_updated_at = NULL, updated_at = clock_timestamp()
          WHERE domain IS NOT NULL
            AND (lower(trim(trailing '.' FROM (CASE WHEN left(domain, 1) = '['
                     THEN split_part(domain, ']', 1) || ']' ELSE split_part(domain, ':', 1) END))) = lower($1)
              OR lower(trim(trailing '.' FROM (CASE WHEN left(domain, 1) = '['
                     THEN split_part(domain, ']', 1) || ']' ELSE split_part(domain, ':', 1) END))) LIKE '%.' || lower($1))",
    )
    .bind(&domain)
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        "UPDATE media_attachments SET
            file_file_name = NULL, file_content_type = NULL, file_file_size = NULL,
            file_updated_at = NULL, thumbnail_file_name = NULL, thumbnail_content_type = NULL,
            thumbnail_file_size = NULL, thumbnail_updated_at = NULL, updated_at = clock_timestamp()
          WHERE account_id IN (
            SELECT id FROM accounts
             WHERE domain IS NOT NULL
               AND (lower(trim(trailing '.' FROM (CASE WHEN left(domain, 1) = '['
                        THEN split_part(domain, ']', 1) || ']' ELSE split_part(domain, ':', 1) END))) = lower($1)
                   OR lower(trim(trailing '.' FROM (CASE WHEN left(domain, 1) = '['
                        THEN split_part(domain, ']', 1) || ']' ELSE split_part(domain, ':', 1) END))) LIKE '%.' || lower($1))
          )",
    )
    .bind(&domain)
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        "DELETE FROM custom_emojis
          WHERE domain IS NOT NULL
            AND (lower(trim(trailing '.' FROM domain)) = lower($1)
              OR lower(trim(trailing '.' FROM domain)) LIKE '%.' || lower($1))",
    )
    .bind(&domain)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}
