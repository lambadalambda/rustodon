//! Remote Note parsing, quote data, media and insertion helpers.

#[allow(clippy::wildcard_imports)] // shares the parent module namespace
use super::*;

pub(super) struct RemoteNoteData {
    pub(super) uri: String,
    pub(super) atom_uri: Option<String>,
    pub(super) url: Option<String>,
    pub(super) content: String,
    pub(super) summary: String,
    pub(super) language: Option<String>,
    pub(super) sensitive: bool,
    pub(super) quote_approval_policy: i32,
    pub(super) published_at: NaiveDateTime,
    pub(super) updated_at: NaiveDateTime,
    pub(super) edited_at: Option<NaiveDateTime>,
    pub(super) in_reply_to_uri: Option<String>,
    pub(super) conversation_uri: Option<String>,
    pub(super) audience: RemoteNoteAudience,
    pub(super) mentions: Vec<String>,
    pub(super) hashtags: Vec<String>,
    pub(super) attachments: Vec<RemoteNoteAttachment>,
    pub(super) favourites_count: Option<i64>,
    pub(super) reblogs_count: Option<i64>,
    pub(super) poll: Option<RemotePollData>,
    pub(super) quote: Option<RemoteQuoteData>,
}

pub(super) struct RemoteQuoteImportGuard<'a> {
    pub(super) request_uri: &'a str,
    pub(super) quoted_status_uri: &'a str,
    pub(super) instrument_uri: &'a str,
    pub(super) expected_target_status_id: i64,
    pub(super) expected_target_account_id: i64,
}

pub(super) struct RemoteQuoteData {
    pub(super) target_uri: Option<String>,
    pub(super) authorization_uri: Option<String>,
    pub(super) legacy: bool,
    pub(super) deleted: bool,
}

pub(super) struct RemoteQuoteAuthorizationData {
    pub(super) uri: String,
    pub(super) attributed_to: Option<String>,
    pub(super) interacting_object: Option<String>,
    pub(super) interaction_target: Option<String>,
    pub(super) typed: bool,
}

pub(super) struct RemotePollData {
    pub(super) options: Vec<String>,
    pub(super) tallies: Vec<i64>,
    pub(super) multiple: bool,
    pub(super) expires_at: Option<NaiveDateTime>,
    pub(super) voters_count: Option<i64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RemoteUpdateAuthority {
    Inbox,
    SignedRefresh,
}

impl RemoteUpdateAuthority {
    pub(super) const fn rejects_tally_regression(self) -> bool {
        matches!(self, Self::Inbox)
    }

    pub(super) const fn claims_freshness(self) -> bool {
        matches!(self, Self::SignedRefresh)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RemotePollReconcile {
    Unchanged,
    Tally(NaiveDateTime),
    Significant,
}

pub(super) struct RemoteNoteAudience {
    pub(super) to: Vec<String>,
    pub(super) cc: Vec<String>,
}

pub(super) struct RemoteNoteAttachment {
    pub(super) remote_url: String,
    pub(super) thumbnail_remote_url: Option<String>,
    pub(super) content_type: Option<String>,
    pub(super) description: Option<String>,
    pub(super) blurhash: Option<String>,
    pub(super) file_meta: Value,
}

impl RemoteNoteData {
    pub(super) fn parse(object: &Value, actor_uri: &str) -> Result<Self, WriteError> {
        let object = object
            .as_object()
            .ok_or(WriteError::InvalidInput("remote Note object is invalid"))?;
        let is_note = equals_or_includes(object.get("type"), "Note");
        let is_question = equals_or_includes(object.get("type"), "Question");
        if !is_note && !is_question {
            return Err(WriteError::InvalidInput(
                "remote object is not a Note or Question",
            ));
        }
        let uri = remote_note_uri(object.get("id"))?
            .ok_or(WriteError::InvalidInput("remote Note object has no ID"))?;
        let attributed_to = remote_note_attributed_to(object.get("attributedTo"))?
            .ok_or(WriteError::InvalidInput("remote Note has no author"))?;
        if attributed_to != actor_uri {
            return Err(WriteError::InvalidInput(
                "remote Note author does not match its signer",
            ));
        }
        let content = object
            .get("content")
            .and_then(Value::as_str)
            .or_else(|| {
                object
                    .get("contentMap")
                    .and_then(Value::as_object)
                    .and_then(|values| values.values().find_map(Value::as_str))
            })
            .filter(|value| value.chars().count() <= 20 * 1024)
            .ok_or(WriteError::InvalidInput("remote Note content is invalid"))?
            .to_owned();
        let content_map = object.get("contentMap").and_then(Value::as_object);
        let language = object
            .get("language")
            .and_then(Value::as_str)
            .or_else(|| content_map.and_then(|values| values.keys().next().map(String::as_str)))
            .filter(|value| !value.trim().is_empty())
            .map(ToOwned::to_owned);
        let published_at = remote_note_timestamp(object, "published", Utc::now().naive_utc())?;
        let edited_at = object
            .get("updated")
            .map(|_| remote_note_timestamp(object, "updated", published_at))
            .transpose()?;
        let updated_at = edited_at.unwrap_or(published_at);
        let audience = RemoteNoteAudience {
            to: remote_note_uri_array(object.get("to"))?,
            cc: remote_note_uri_array(object.get("cc"))?,
        };
        let (mentions, hashtags) = remote_note_tags(object.get("tag"))?;
        let atom_uri = crate::mastodon::activitypub_inbox::optional_atom_uri(object.get("atomUri"))
            .map_err(|_| WriteError::InvalidInput("remote Note atom URI is invalid"))?;
        if let Some(atom_uri) = atom_uri.as_deref()
            && !same_remote_note_host(actor_uri, atom_uri)?
        {
            return Err(WriteError::InvalidInput(
                "remote Note atom URI does not match its actor host",
            ));
        }
        Ok(Self {
            uri,
            atom_uri,
            url: remote_note_optional_uri(object.get("url"))?,
            content,
            summary: object
                .get("summary")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .chars()
                .take(20 * 1024)
                .collect(),
            language,
            sensitive: object
                .get("sensitive")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            quote_approval_policy: remote_quote_approval_policy(object, actor_uri)?,
            published_at,
            updated_at,
            edited_at,
            in_reply_to_uri: remote_note_optional_uri(object.get("inReplyTo"))?,
            conversation_uri: remote_note_optional_conversation_uri(object.get("conversation"))?,
            audience,
            mentions,
            hashtags,
            attachments: remote_note_attachments(object.get("attachment")),
            favourites_count: remote_note_interaction_count(object, "likes", "favouritesCount")?,
            reblogs_count: remote_note_interaction_count(object, "shares", "reblogsCount")?,
            poll: is_question.then(|| remote_poll_data(object)).transpose()?,
            quote: remote_quote_data(object)?,
        })
    }
}

pub(super) fn remote_quote_approval_policy(
    object: &serde_json::Map<String, Value>,
    actor_uri: &str,
) -> Result<i32, WriteError> {
    let actor_uri = actor_uri.trim_end_matches('/');
    remote_quote_approval_policy_with_collections(
        object,
        actor_uri,
        &format!("{actor_uri}/followers"),
        &format!("{actor_uri}/following"),
    )
}

pub(super) fn remote_quote_approval_policy_with_collections(
    object: &serde_json::Map<String, Value>,
    actor_uri: &str,
    followers_uri: &str,
    following_uri: &str,
) -> Result<i32, WriteError> {
    let Some(policy) = object
        .get("interactionPolicy")
        .and_then(Value::as_object)
        .and_then(|policy| policy.get("canQuote"))
        .and_then(Value::as_object)
    else {
        return Ok(0);
    };
    let automatic = remote_quote_subpolicy(
        policy.get("automaticApproval"),
        actor_uri,
        followers_uri,
        following_uri,
    )?;
    let manual = remote_quote_subpolicy(
        policy.get("manualApproval"),
        actor_uri,
        followers_uri,
        following_uri,
    )?;
    Ok((automatic << 16) | manual)
}

pub(super) fn remote_quote_subpolicy(
    value: Option<&Value>,
    actor_uri: &str,
    followers_uri: &str,
    following_uri: &str,
) -> Result<i32, WriteError> {
    let values = match value {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(values)) if values.len() <= 100 => values.iter().collect(),
        Some(Value::Array(_)) => {
            return Err(WriteError::InvalidInput(
                "remote quote interaction policy is too large",
            ));
        }
        Some(value) => vec![value],
    };
    let actor_uri = actor_uri.trim_end_matches('/');
    Ok(values.into_iter().fold(0, |flags, value| {
        let uri = value
            .as_str()
            .or_else(|| value.as_object()?.get("id")?.as_str());
        flags
            | match uri {
                Some("as:Public" | "Public" | "https://www.w3.org/ns/activitystreams#Public") => 2,
                Some(uri) if uri == followers_uri => 4,
                Some(uri) if uri == following_uri => 8,
                Some(uri) if uri == actor_uri => 0,
                _ => 1,
            }
    }))
}

pub(super) fn remote_quote_authorization_data(
    value: &Value,
) -> Result<RemoteQuoteAuthorizationData, WriteError> {
    let uri = remote_note_uri(Some(value))?.ok_or(WriteError::InvalidInput(
        "remote quote authorization has no ID",
    ))?;
    let embedded = value.as_object();
    Ok(RemoteQuoteAuthorizationData {
        uri,
        attributed_to: embedded
            .map(|value| remote_note_optional_uri(value.get("attributedTo")))
            .transpose()?
            .flatten(),
        interacting_object: embedded
            .map(|value| remote_note_optional_uri(value.get("interactingObject")))
            .transpose()?
            .flatten(),
        interaction_target: embedded
            .map(|value| remote_note_optional_uri(value.get("interactionTarget")))
            .transpose()?
            .flatten(),
        typed: embedded
            .is_some_and(|value| equals_or_includes(value.get("type"), "QuoteAuthorization")),
    })
}

pub(super) fn remote_quote_data(
    object: &serde_json::Map<String, Value>,
) -> Result<Option<RemoteQuoteData>, WriteError> {
    let Some((field, value)) = ["quote", "_misskey_quote", "quoteUrl", "quoteUri"]
        .into_iter()
        .find_map(|field| object.get(field).map(|value| (field, value)))
    else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let legacy = field != "quote";
    let deleted = value
        .as_object()
        .is_some_and(|quote| equals_or_includes(quote.get("type"), "Tombstone"));
    if let Some(quote) = value.as_object()
        && !deleted
        && !equals_or_includes(quote.get("type"), "Note")
        && !equals_or_includes(quote.get("type"), "Question")
    {
        return Err(WriteError::InvalidInput(
            "remote quote object type is invalid",
        ));
    }
    let target_uri = if deleted
        && value
            .as_object()
            .is_some_and(|quote| quote.get("id").or_else(|| quote.get("href")).is_none())
    {
        None
    } else {
        remote_note_uri(Some(value))?
    };
    if target_uri.is_none() && !deleted {
        return Err(WriteError::InvalidInput("remote quote has no target"));
    }
    let authorization = object
        .get("quoteAuthorization")
        .and_then(|value| {
            value
                .as_array()
                .and_then(|values| values.first())
                .or(Some(value))
        })
        .filter(|value| !value.is_null())
        .map(remote_quote_authorization_data)
        .transpose()?;
    Ok(Some(RemoteQuoteData {
        target_uri,
        authorization_uri: authorization.map(|authorization| authorization.uri),
        legacy,
        deleted,
    }))
}

pub(super) fn remote_poll_data(
    object: &serde_json::Map<String, Value>,
) -> Result<RemotePollData, WriteError> {
    let (multiple, values) = if let Some(Value::Array(values)) = object.get("anyOf") {
        (true, values)
    } else if let Some(Value::Array(values)) = object.get("oneOf") {
        (false, values)
    } else {
        return Err(WriteError::InvalidInput(
            "remote Question options are invalid",
        ));
    };
    let mut options = Vec::new();
    let mut tallies = Vec::new();
    for value in values.iter().take(500) {
        let option = value.as_object().ok_or(WriteError::InvalidInput(
            "remote Question option is invalid",
        ))?;
        if let Some(title) = option
            .get("name")
            .and_then(Value::as_str)
            .filter(|title| !title.trim().is_empty())
            .or_else(|| {
                option
                    .get("content")
                    .and_then(Value::as_str)
                    .filter(|title| !title.trim().is_empty())
            })
        {
            options.push(title.to_owned());
        }
        tallies.push(
            option
                .get("replies")
                .and_then(Value::as_object)
                .and_then(|replies| replies.get("totalItems"))
                .and_then(Value::as_i64)
                .unwrap_or(0)
                .max(0),
        );
    }
    if options.is_empty() {
        return Err(WriteError::InvalidInput(
            "remote Question options are empty",
        ));
    }
    let closed = object.get("closed");
    let expires_at = match closed {
        Some(Value::String(value)) => DateTime::parse_from_rfc3339(value)
            .ok()
            .map(|value| value.naive_utc()),
        Some(Value::Bool(true) | Value::Number(_) | Value::Array(_) | Value::Object(_)) => {
            Some(Utc::now().naive_utc())
        }
        None | Some(Value::Null | Value::Bool(false)) => object
            .get("endTime")
            .and_then(Value::as_str)
            .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
            .map(|value| value.naive_utc()),
    };
    Ok(RemotePollData {
        options,
        tallies,
        multiple,
        expires_at,
        voters_count: object
            .get("votersCount")
            .and_then(Value::as_i64)
            .map(|count| count.max(0)),
    })
}

pub(super) async fn upsert_remote_emojis(
    transaction: &mut Transaction<'_, Postgres>,
    domain: &str,
    actor_uri: &str,
    object: &Value,
) -> Result<(), WriteError> {
    let domain = canonical_remote_domain(domain)
        .map_err(|_| WriteError::InvalidInput("remote emoji domain is invalid"))?;
    for emoji in parse_note_emojis(object, actor_uri) {
        sqlx::query(
            "SELECT pg_catalog.pg_advisory_xact_lock(
                 pg_catalog.hashtextextended($1 || ':' || $2, 0)
             )",
        )
        .bind(&domain)
        .bind(&emoji.shortcode)
        .execute(&mut **transaction)
        .await?;
        let existing = sqlx::query_as::<_, (i64, Option<String>, Option<String>, NaiveDateTime)>(
            "SELECT id, image_remote_url, image_file_name, updated_at
             FROM custom_emojis WHERE shortcode = $1 AND domain = $2 FOR UPDATE",
        )
        .bind(&emoji.shortcode)
        .bind(&domain)
        .fetch_optional(&mut **transaction)
        .await?;
        let (emoji_id, should_fetch) =
            if let Some((id, remote_url, file_name, updated_at)) = existing {
                let changed_url = remote_url.as_deref() != Some(emoji.image_url.as_str());
                let fresh = emoji
                    .updated_at
                    .is_some_and(|updated| updated >= updated_at);
                let (update_metadata, should_fetch) =
                    remote_emoji_update_decision(changed_url, fresh, file_name.is_some());
                if !update_metadata && !should_fetch {
                    continue;
                }
                if update_metadata {
                    sqlx::query(
                        "UPDATE custom_emojis SET image_remote_url = $2,
                         uri = COALESCE($3, uri), updated_at = clock_timestamp()
                     WHERE id = $1",
                    )
                    .bind(id)
                    .bind(&emoji.image_url)
                    .bind(&emoji.uri)
                    .execute(&mut **transaction)
                    .await?;
                }
                (id, should_fetch)
            } else {
                let id = sqlx::query_scalar::<_, i64>(
                    "INSERT INTO custom_emojis
                    (shortcode, domain, uri, image_remote_url, disabled, visible_in_picker,
                     created_at, updated_at)
                 VALUES ($1, $2, $3, $4, false, true, clock_timestamp(), clock_timestamp())
                 ON CONFLICT (shortcode, domain) DO UPDATE SET updated_at = custom_emojis.updated_at
                 RETURNING id",
                )
                .bind(&emoji.shortcode)
                .bind(&domain)
                .bind(&emoji.uri)
                .bind(&emoji.image_url)
                .fetch_one(&mut **transaction)
                .await?;
                (id, true)
            };
        if should_fetch {
            let digest = Sha256::digest(emoji.image_url.as_bytes());
            let job = JobSpec::new(
                Lane::Pull,
                ACTIVITYPUB_EMOJI_FETCH_JOB_KIND,
                json!({
                    "emoji_id": emoji_id,
                    "remote_url": emoji.image_url,
                    "media_type": emoji.media_type,
                    "domain": domain
                }),
            )
            .logical_key(format!("activitypub:emoji:{emoji_id}:{digest:x}"))
            .max_attempts(4);
            record_outbox_in(transaction, &job).await?;
        }
    }
    Ok(())
}

pub(super) fn remote_emoji_update_decision(
    changed_url: bool,
    fresh_timestamp: bool,
    has_file: bool,
) -> (bool, bool) {
    // Mastodon accepts fresher metadata at the same URL without downloading an installed file again.
    (changed_url || fresh_timestamp, changed_url || !has_file)
}

pub(super) async fn lock_remote_note(
    transaction: &mut Transaction<'_, Postgres>,
    uri: &str,
) -> Result<(), WriteError> {
    sqlx::query(
        "SELECT pg_catalog.pg_advisory_xact_lock(
            pg_catalog.hashtextextended($1, 0)
         )",
    )
    .bind(uri)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

pub(super) async fn lock_remote_interaction(
    transaction: &mut Transaction<'_, Postgres>,
    activity_uri: &str,
) -> Result<(), WriteError> {
    sqlx::query(
        "SELECT pg_catalog.pg_advisory_xact_lock(
            pg_catalog.hashtextextended($1, 0)
         )",
    )
    .bind(activity_uri)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

pub(super) async fn remote_quote_request_decision_in(
    transaction: &mut Transaction<'_, Postgres>,
    request_uri: &str,
    actor_uri: &str,
    quoted_status_uri: &str,
    instrument_uri: &str,
) -> Result<Option<bool>, WriteError> {
    let logical_key = activitypub::quote_request_decision_logical_key(request_uri);
    let body = sqlx::query_scalar::<_, Value>(
        "SELECT payload -> 'arguments' -> 'body' FROM rustodon.outbox_events \
         WHERE kind = $1 AND logical_key = $2",
    )
    .bind(ACTIVITYPUB_DELIVERY_JOB_KIND)
    .bind(logical_key)
    .fetch_optional(&mut **transaction)
    .await?;
    let Some(body) = body else {
        return Ok(None);
    };
    let accepted = match body.get("type").and_then(Value::as_str) {
        Some("Accept") => true,
        Some("Reject") => false,
        _ => return Err(WriteError::Conflict),
    };
    let request = body
        .get("object")
        .and_then(Value::as_object)
        .ok_or(WriteError::Conflict)?;
    if request.get("id").and_then(Value::as_str) != Some(request_uri)
        || request.get("actor").and_then(Value::as_str) != Some(actor_uri)
        || request.get("object").and_then(Value::as_str) != Some(quoted_status_uri)
        || request.get("instrument").and_then(Value::as_str) != Some(instrument_uri)
    {
        return Err(WriteError::Conflict);
    }
    Ok(Some(accepted))
}

pub(super) async fn remote_interaction_actor_matches(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
    actor_uri: &str,
    require_active: bool,
) -> Result<bool, WriteError> {
    let Some((domain, current_uri, suspended_at)) =
        sqlx::query_as::<_, (Option<String>, String, Option<NaiveDateTime>)>(
            "SELECT domain, uri, suspended_at FROM accounts WHERE id = $1 FOR UPDATE",
        )
        .bind(account_id)
        .fetch_optional(&mut **transaction)
        .await?
    else {
        return Ok(false);
    };
    Ok(domain.is_some() && current_uri == actor_uri && (!require_active || suspended_at.is_none()))
}

pub(super) async fn local_interaction_target(
    transaction: &mut Transaction<'_, Postgres>,
    object_uri: &str,
    origin: &str,
) -> Result<Option<(i64, i64, i32)>, WriteError> {
    let origin = origin.trim_end_matches('/');
    Ok(sqlx::query_as::<_, (i64, i64, i32)>(
        "SELECT status.id, status.account_id, status.visibility
           FROM statuses status
           JOIN accounts author ON author.id = status.account_id
                              AND author.domain IS NULL
          WHERE status.deleted_at IS NULL
            AND (
              status.uri = $1
              OR status.url = $1
              OR $1 = $2 || '/actor/statuses/' || status.id::text
                   || CASE WHEN status.reblog_of_id IS NULL THEN '' ELSE '/activity' END
              OR $1 = $2 || '/@' || author.username || '/' || status.id::text
                   || CASE WHEN status.reblog_of_id IS NULL THEN '' ELSE '/activity' END
              OR $1 = $2 || '/users/' || author.username || '/statuses/' || status.id::text
                   || CASE WHEN status.reblog_of_id IS NULL THEN '' ELSE '/activity' END
              OR $1 = $2 || '/ap/users/' || author.id::text || '/statuses/' || status.id::text
                   || CASE WHEN status.reblog_of_id IS NULL THEN '' ELSE '/activity' END
            )
          ORDER BY status.id
          LIMIT 1 FOR UPDATE",
    )
    .bind(object_uri)
    .bind(origin)
    .fetch_optional(&mut **transaction)
    .await?)
}

pub(super) async fn announce_interaction_target(
    transaction: &mut Transaction<'_, Postgres>,
    object_uri: &str,
    origin: &str,
) -> Result<Option<(i64, i64, i32, bool, bool)>, WriteError> {
    if let Some((status_id, _, _)) =
        local_interaction_target(transaction, object_uri, origin).await?
    {
        return announce_target_from_status(transaction, status_id, true).await;
    }

    let matched_status_id = sqlx::query_scalar::<_, i64>(
        "SELECT status.id
           FROM statuses status
           JOIN accounts author ON author.id = status.account_id
                              AND author.domain IS NOT NULL
           WHERE status.deleted_at IS NULL
             AND (status.uri = $1 OR status.url = $1)
           ORDER BY status.id
           LIMIT 1",
    )
    .bind(object_uri)
    .fetch_optional(&mut **transaction)
    .await?;
    let Some(matched_status_id) = matched_status_id else {
        return Ok(None);
    };
    announce_target_from_status(transaction, matched_status_id, false).await
}

pub(super) async fn announce_target_from_status(
    transaction: &mut Transaction<'_, Postgres>,
    matched_status_id: i64,
    matched_account_is_local: bool,
) -> Result<Option<(i64, i64, i32, bool, bool)>, WriteError> {
    let target = sqlx::query_as::<_, (i64, i64, i32, bool)>(
        "SELECT target.id, target.account_id, target.visibility, target_author.domain IS NULL
           FROM statuses matched
           JOIN statuses target ON target.id = COALESCE(matched.reblog_of_id, matched.id)
           JOIN accounts target_author ON target_author.id = target.account_id
          WHERE matched.id = $1 AND matched.deleted_at IS NULL AND target.deleted_at IS NULL
          FOR UPDATE OF target",
    )
    .bind(matched_status_id)
    .fetch_optional(&mut **transaction)
    .await?;
    if target.is_some()
        && sqlx::query_scalar::<_, i64>(
            "SELECT id FROM statuses WHERE id = $1 AND deleted_at IS NULL FOR UPDATE",
        )
        .bind(matched_status_id)
        .fetch_optional(&mut **transaction)
        .await?
        .is_none()
    {
        return Ok(None);
    }
    Ok(target.map(
        |(target_status_id, recipient_account_id, target_visibility, original_account_is_local)| {
            (
                target_status_id,
                recipient_account_id,
                target_visibility,
                matched_account_is_local,
                original_account_is_local,
            )
        },
    ))
}

pub(super) async fn remote_announce_is_relevant(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
) -> Result<bool, WriteError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1 FROM follows follow
            JOIN accounts local_account ON local_account.id = follow.account_id
                                         AND local_account.domain IS NULL
            WHERE follow.target_account_id = $1
        )",
    )
    .bind(account_id)
    .fetch_one(&mut **transaction)
    .await?)
}

pub(super) async fn remote_announce_notification_suppressed(
    transaction: &mut Transaction<'_, Postgres>,
    reblogger_account_id: i64,
    original_author_account_id: i64,
) -> Result<bool, WriteError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1
              FROM accounts reblogger
              JOIN follows follow ON follow.target_account_id = reblogger.id
             WHERE reblogger.id = $1
               AND reblogger.actor_type = 'Group'
               AND follow.account_id = $2
        )",
    )
    .bind(reblogger_account_id)
    .bind(original_author_account_id)
    .fetch_one(&mut **transaction)
    .await?)
}

pub(super) fn remote_interaction_visibility(
    to: &[String],
    cc: &[String],
    followers_url: &str,
) -> i32 {
    if to.iter().any(|uri| activitypub::is_public_address(uri)) {
        return 0;
    }
    if cc.iter().any(|uri| activitypub::is_public_address(uri)) {
        return 1;
    }
    if to
        .iter()
        .any(|uri| uri == followers_url && !followers_url.is_empty())
    {
        return 2;
    }
    3
}

pub(super) fn remote_interaction_timestamp(
    published_at: Option<&str>,
) -> Result<NaiveDateTime, WriteError> {
    let Some(published_at) = published_at else {
        return Ok(Utc::now().naive_utc());
    };
    let timestamp = DateTime::parse_from_rfc3339(published_at)
        .map(|timestamp| timestamp.naive_utc())
        .map_err(|_| WriteError::InvalidInput("remote Announce timestamp is invalid"))?;
    if timestamp > Utc::now().naive_utc() + ChronoDuration::hours(24) {
        return Err(WriteError::InvalidInput(
            "remote Announce timestamp is too far in the future",
        ));
    }
    Ok(timestamp)
}

pub(super) async fn remote_interaction_tombstoned(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
    activity_uri: &str,
) -> Result<bool, WriteError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM tombstones WHERE account_id = $1 AND uri = $2)",
    )
    .bind(account_id)
    .bind(activity_uri)
    .fetch_one(&mut **transaction)
    .await?)
}

pub(super) async fn remote_note_status(
    transaction: &mut Transaction<'_, Postgres>,
    uri: &str,
    atom_uri: Option<&str>,
) -> Result<
    Option<(
        i64,
        i64,
        Option<NaiveDateTime>,
        Option<NaiveDateTime>,
        NaiveDateTime,
    )>,
    WriteError,
> {
    Ok(sqlx::query_as::<
        _,
        (
            i64,
            i64,
            Option<NaiveDateTime>,
            Option<NaiveDateTime>,
            NaiveDateTime,
        ),
    >(
        "SELECT id, account_id, deleted_at, edited_at, created_at FROM statuses
         WHERE uri = $1 OR ($2::text IS NOT NULL AND uri = $2)
         ORDER BY id LIMIT 1 FOR UPDATE",
    )
    .bind(uri)
    .bind(atom_uri)
    .fetch_optional(&mut **transaction)
    .await?)
}

pub(super) async fn remote_note_status_id_for_account(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
    uri: &str,
    atom_uri: Option<&str>,
) -> Result<Option<i64>, WriteError> {
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT id FROM statuses
          WHERE account_id = $1
            AND (uri = $2 OR ($3::text IS NOT NULL AND uri = $3))
          ORDER BY id LIMIT 1",
    )
    .bind(account_id)
    .bind(uri)
    .bind(atom_uri)
    .fetch_optional(&mut **transaction)
    .await?)
}

pub(super) async fn remote_note_tombstoned(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
    uri: &str,
    atom_uri: Option<&str>,
) -> Result<bool, WriteError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM tombstones
         WHERE account_id = $1 AND (uri = $2 OR ($3::text IS NOT NULL AND uri = $3)))",
    )
    .bind(account_id)
    .bind(uri)
    .bind(atom_uri)
    .fetch_one(&mut **transaction)
    .await?)
}

pub(super) async fn insert_remote_note_tombstone(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
    uri: &str,
) -> Result<(), WriteError> {
    sqlx::query(
        "INSERT INTO tombstones (account_id, uri, by_moderator, created_at, updated_at)
         SELECT $1, $2, false, clock_timestamp(), clock_timestamp()
         WHERE NOT EXISTS (SELECT 1 FROM tombstones WHERE account_id = $1 AND uri = $2)",
    )
    .bind(account_id)
    .bind(uri)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

pub(super) fn same_remote_note_host(actor_uri: &str, object_uri: &str) -> Result<bool, WriteError> {
    let actor = Url::parse(actor_uri)
        .map_err(|_| WriteError::InvalidInput("remote actor URI is invalid"))?;
    let object = Url::parse(object_uri)
        .map_err(|_| WriteError::InvalidInput("remote object URI is invalid"))?;
    Ok(actor
        .host_str()
        .zip(object.host_str())
        .is_some_and(|(actor_host, object_host)| actor_host.eq_ignore_ascii_case(object_host)))
}

pub(super) fn remote_note_visibility(audience: &RemoteNoteAudience, followers_url: &str) -> i32 {
    if audience
        .to
        .iter()
        .any(|uri| activitypub::is_public_address(uri))
    {
        return 0;
    }
    if audience
        .cc
        .iter()
        .any(|uri| activitypub::is_public_address(uri))
    {
        return 1;
    }
    if audience
        .to
        .iter()
        .any(|uri| uri == followers_url && !followers_url.is_empty())
    {
        return 2;
    }
    4
}

pub(super) async fn remote_note_has_only_explicit_recipients(
    transaction: &mut Transaction<'_, Postgres>,
    status_id: i64,
    note: &RemoteNoteData,
) -> Result<bool, WriteError> {
    // Local mentions already resolve actor aliases and include the delivery target.
    // Known remote audience accounts also count, but unknown URIs and collections
    // must not manufacture silent recipients or trigger remote resolution here.
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1 FROM mentions WHERE status_id = $1 AND silent IS FALSE
            UNION ALL
            SELECT 1 FROM accounts WHERE domain IS NOT NULL AND uri = ANY($2::text[])
         ) AND NOT EXISTS (
            SELECT 1 FROM mentions WHERE status_id = $1 AND silent IS TRUE
            UNION ALL
            SELECT 1 FROM accounts
             WHERE domain IS NOT NULL
               AND (uri = ANY($3::text[]) OR uri = ANY($4::text[]))
               AND NOT (uri = ANY($2::text[]))
         )",
    )
    .bind(status_id)
    .bind(&note.mentions)
    .bind(&note.audience.to)
    .bind(&note.audience.cc)
    .fetch_one(&mut **transaction)
    .await?)
}

pub(super) async fn remote_note_thread(
    transaction: &mut Transaction<'_, Postgres>,
    note: &RemoteNoteData,
    origin: &str,
) -> Result<(Option<i64>, Option<i64>, Option<i64>), WriteError> {
    let Some(parent_uri) = note.in_reply_to_uri.as_deref() else {
        return Ok((None, None, None));
    };
    let parent = sqlx::query_as::<_, (i64, i64, Option<i64>)>(
        "SELECT status.id, status.account_id, status.conversation_id
           FROM statuses status
           JOIN accounts author ON author.id = status.account_id
          WHERE status.deleted_at IS NULL
            AND (
              status.uri = $1
              OR status.url = $1
              OR (author.domain IS NULL AND (
                $1 = $2 || '/actor/statuses/' || status.id::text
                     || CASE WHEN status.reblog_of_id IS NULL THEN '' ELSE '/activity' END
                OR $1 = $2 || '/@' || author.username || '/' || status.id::text
                     || CASE WHEN status.reblog_of_id IS NULL THEN '' ELSE '/activity' END
                OR $1 = $2 || '/users/' || author.username || '/statuses/' || status.id::text
                     || CASE WHEN status.reblog_of_id IS NULL THEN '' ELSE '/activity' END
                OR $1 = $2 || '/ap/users/' || author.id::text || '/statuses/' || status.id::text
                     || CASE WHEN status.reblog_of_id IS NULL THEN '' ELSE '/activity' END
              ))
            )
          ORDER BY status.id
          LIMIT 1
          FOR UPDATE",
    )
    .bind(parent_uri)
    .bind(origin.trim_end_matches('/'))
    .fetch_optional(&mut **transaction)
    .await?;
    Ok(
        parent.map_or((None, None, None), |(id, account_id, conversation_id)| {
            (Some(id), Some(account_id), conversation_id)
        }),
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn local_actor_uri_for_account(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
    origin: &str,
) -> Result<String, WriteError> {
    sqlx::query_scalar::<_, String>(
        "SELECT CASE WHEN id = -99 THEN $2 || '/actor' \
                     WHEN id_scheme = 1 THEN $2 || '/ap/users/' || id::text \
                     ELSE $2 || '/users/' || username END \
           FROM accounts WHERE id = $1 AND domain IS NULL",
    )
    .bind(account_id)
    .bind(origin.trim_end_matches('/'))
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(WriteError::NotFound)
}

pub(super) async fn quote_target_matches_uri(
    transaction: &mut Transaction<'_, Postgres>,
    status_id: i64,
    uri: &str,
    origin: &str,
) -> Result<bool, WriteError> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM statuses status \
           JOIN accounts author ON author.id = status.account_id \
          WHERE status.id = $1 AND status.deleted_at IS NULL AND ( \
                status.uri = $2 OR status.url = $2 OR (author.domain IS NULL AND ( \
                $2 = $3 || '/actor/statuses/' || status.id::text \
                OR $2 = $3 || '/@' || author.username || '/' || status.id::text \
                OR $2 = $3 || '/users/' || author.username || '/statuses/' || status.id::text \
                OR $2 = $3 || '/ap/users/' || author.id::text || '/statuses/' || status.id::text))))",
    )
    .bind(status_id)
    .bind(uri)
    .bind(origin.trim_end_matches('/'))
    .fetch_one(&mut **transaction)
    .await?)
}

pub(super) async fn resolve_quote_target(
    transaction: &mut Transaction<'_, Postgres>,
    target_uri: &str,
    origin: &str,
) -> Result<Option<(i64, i64, bool, String)>, WriteError> {
    Ok(sqlx::query_as::<_, (i64, i64, bool, String)>(
        "SELECT status.id, status.account_id, author.domain IS NULL, \
                CASE WHEN author.domain IS NULL THEN \
                  CASE WHEN author.id = -99 THEN $2 || '/actor' \
                       WHEN author.id_scheme = 1 THEN $2 || '/ap/users/' || author.id::text \
                       ELSE $2 || '/users/' || author.username END \
                ELSE author.uri END \
           FROM statuses status \
           JOIN accounts author ON author.id = status.account_id \
          WHERE status.deleted_at IS NULL AND status.reblog_of_id IS NULL AND ( \
                status.uri = $1 OR status.url = $1 OR (author.domain IS NULL AND ( \
                $1 = $2 || '/actor/statuses/' || status.id::text \
                OR $1 = $2 || '/@' || author.username || '/' || status.id::text \
                OR $1 = $2 || '/users/' || author.username || '/statuses/' || status.id::text \
                OR $1 = $2 || '/ap/users/' || author.id::text || '/statuses/' || status.id::text))) \
          ORDER BY status.id LIMIT 1",
    )
    .bind(target_uri)
    .bind(origin.trim_end_matches('/'))
    .fetch_optional(&mut **transaction)
    .await?)
}

pub(super) async fn quote_target_id_for_status(
    transaction: &mut Transaction<'_, Postgres>,
    status_id: i64,
) -> Result<Option<i64>, WriteError> {
    Ok(sqlx::query_scalar::<_, Option<i64>>(
        "SELECT quoted_status_id FROM quotes WHERE status_id = $1",
    )
    .bind(status_id)
    .fetch_optional(&mut **transaction)
    .await?
    .flatten())
}

pub(super) async fn lock_quote_status_deletion(
    transaction: &mut Transaction<'_, Postgres>,
) -> Result<(), WriteError> {
    // Discovering every status in a quote component requires locking one endpoint first.
    // Serialize status deletions so two endpoint removals cannot each hold that first row
    // while waiting for the other's quote/status lock.
    sqlx::query("SELECT pg_advisory_xact_lock(7640897321347509341::bigint)")
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

pub(super) async fn lock_statuses_in_order(
    transaction: &mut Transaction<'_, Postgres>,
    status_ids: &[i64],
) -> Result<(), WriteError> {
    let mut status_ids = status_ids.to_vec();
    status_ids.sort_unstable();
    status_ids.dedup();
    if !status_ids.is_empty() {
        sqlx::query_scalar::<_, i64>(
            "SELECT id FROM statuses WHERE id = ANY($1) ORDER BY id FOR UPDATE",
        )
        .bind(status_ids)
        .fetch_all(&mut **transaction)
        .await?;
    }
    Ok(())
}

pub(super) async fn record_remote_quote_authorization_forwarding_in(
    transaction: &mut Transaction<'_, Postgres>,
    source_account_id: i64,
    actor_uri: &str,
    status_id: i64,
    activity: &Value,
) -> Result<(), WriteError> {
    let (visibility, parent_account_id, source_inbox) =
        sqlx::query_as::<_, (i32, Option<i64>, String)>(
            "SELECT status.visibility, parent.id,
                    COALESCE(NULLIF(source.shared_inbox_url, ''), source.inbox_url)
               FROM statuses status
               JOIN accounts source ON source.id = $2 AND source.uri = $3
                                   AND source.domain IS NOT NULL
          LEFT JOIN statuses parent_status ON parent_status.id = status.in_reply_to_id
                                           AND parent_status.deleted_at IS NULL
          LEFT JOIN accounts parent ON parent.id = parent_status.account_id
                                    AND parent.domain IS NULL
              WHERE status.id = $1 AND status.deleted_at IS NULL",
        )
        .bind(status_id)
        .bind(source_account_id)
        .bind(actor_uri)
        .fetch_one(&mut **transaction)
        .await?;
    if !matches!(visibility, 0 | 1) {
        return Ok(());
    }
    let activity_uri = activity
        .get("id")
        .and_then(Value::as_str)
        .filter(|uri| !uri.trim().is_empty())
        .ok_or(WriteError::InvalidInput("signed remote activity has no ID"))?;
    record_remote_activity_forwarding_for_status_in(
        transaction,
        status_id,
        parent_account_id,
        &source_inbox,
        activity_uri,
        activity,
    )
    .await
}

pub(super) async fn record_remote_activity_forwarding_for_status_in(
    transaction: &mut Transaction<'_, Postgres>,
    status_id: i64,
    parent_account_id: Option<i64>,
    source_inbox: &str,
    activity_uri: &str,
    activity: &Value,
) -> Result<(), WriteError> {
    let shared_account_ids = sqlx::query_scalar::<_, i64>(
        "SELECT shared.account_id
           FROM (
             SELECT reblog.account_id, 0 AS share_kind, reblog.id AS share_id
               FROM statuses reblog
               JOIN accounts account ON account.id = reblog.account_id
                                    AND account.domain IS NULL
              WHERE reblog.reblog_of_id = $1
                AND reblog.deleted_at IS NULL
             UNION ALL
             SELECT quote.account_id, 1 AS share_kind, quote.id AS share_id
               FROM quotes quote
               JOIN accounts account ON account.id = quote.account_id
                                    AND account.domain IS NULL
              WHERE quote.quoted_status_id = $1 AND quote.state = 1
                AND EXISTS (SELECT 1 FROM statuses quoting
                             WHERE quoting.id = quote.status_id
                               AND quoting.deleted_at IS NULL)
           ) shared
          ORDER BY shared.share_kind, shared.share_id DESC, shared.account_id",
    )
    .bind(status_id)
    .fetch_all(&mut **transaction)
    .await?;
    let source_account_id = parent_account_id.or_else(|| shared_account_ids.first().copied());
    let Some(source_account_id) = source_account_id else {
        return Ok(());
    };
    let mut target_account_ids = shared_account_ids;
    if let Some(parent_account_id) = parent_account_id {
        target_account_ids.push(parent_account_id);
    }
    target_account_ids.sort_unstable();
    target_account_ids.dedup();
    let followers = sqlx::query_as::<_, (String, String)>(
        "SELECT DISTINCT
                COALESCE(NULLIF(follower.shared_inbox_url, ''), follower.inbox_url),
                follower.domain
           FROM follows follow
           JOIN accounts follower ON follower.id = follow.account_id
                                 AND follower.domain IS NOT NULL
                                 AND follower.protocol = 1
                                 AND follower.suspended_at IS NULL
          WHERE follow.target_account_id = ANY($1)
            AND COALESCE(NULLIF(follower.shared_inbox_url, ''), follower.inbox_url) <> ''
            AND COALESCE(NULLIF(follower.shared_inbox_url, ''), follower.inbox_url) <> $2
           ORDER BY 1, 2",
    )
    .bind(&target_account_ids)
    .bind(source_inbox)
    .fetch_all(&mut **transaction)
    .await?;
    for (inbox_url, remote_domain) in followers {
        let delivery = JobSpec::new(
            Lane::Push,
            ACTIVITYPUB_DELIVERY_JOB_KIND,
            json!({
                "source_account_id": source_account_id,
                "inbox_url": inbox_url,
                "remote_domain": remote_domain,
                "body": activity
            }),
        )
        .logical_key(activitypub::forward_delivery_logical_key(
            source_account_id,
            activity_uri,
            &inbox_url,
        ));
        record_outbox_once_in(transaction, &delivery).await?;
    }
    Ok(())
}

pub(super) async fn locked_quote_for_status(
    transaction: &mut Transaction<'_, Postgres>,
    status_id: i64,
) -> Result<
    Option<(
        i64,
        Option<i64>,
        Option<i64>,
        i32,
        Option<String>,
        bool,
        Option<String>,
    )>,
    WriteError,
> {
    Ok(sqlx::query_as::<
        _,
        (
            i64,
            Option<i64>,
            Option<i64>,
            i32,
            Option<String>,
            bool,
            Option<String>,
        ),
    >(
        "SELECT id, quoted_status_id, quoted_account_id, state, approval_uri, legacy, \
                    activity_uri \
           FROM quotes WHERE status_id = $1 FOR UPDATE",
    )
    .bind(status_id)
    .fetch_optional(&mut **transaction)
    .await?)
}

pub(super) fn quote_state_update_counter_delta(legacy: bool, old_state: i32, new_state: i32) -> i8 {
    if legacy || old_state == new_state {
        0
    } else if old_state != 1 && new_state == 1 {
        1
    } else if old_state == 1 && new_state != 1 {
        -1
    } else {
        0
    }
}

pub(super) fn reconciled_remote_quote_state(
    target_changed: bool,
    old_state: i32,
    old_approval_uri: Option<&str>,
    advertised_approval_uri: Option<&str>,
    computed_state: i32,
) -> (i32, Option<String>) {
    if target_changed {
        return (computed_state, None);
    }
    if old_state == 1 && old_approval_uri.is_some() && old_approval_uri != advertised_approval_uri {
        (0, None)
    } else {
        (old_state, old_approval_uri.map(str::to_owned))
    }
}

pub(super) async fn prelock_remote_note_quote_targets(
    transaction: &mut Transaction<'_, Postgres>,
    existing_status_id: Option<i64>,
    note: &RemoteNoteData,
    origin: &str,
) -> Result<(), WriteError> {
    let mut target_status_ids = existing_status_id.into_iter().collect::<Vec<_>>();
    if let Some(existing_status_id) = existing_status_id
        && let Some(target_status_id) =
            quote_target_id_for_status(transaction, existing_status_id).await?
    {
        target_status_ids.push(target_status_id);
    }
    if let Some(quote) = note.quote.as_ref()
        && !quote.deleted
        && let Some(target_uri) = quote.target_uri.as_deref()
        && let Some((target_status_id, ..)) =
            resolve_quote_target(transaction, target_uri, origin).await?
    {
        target_status_ids.push(target_status_id);
    }
    lock_statuses_in_order(transaction, &target_status_ids).await
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn insert_reconciled_remote_quote(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
    status_id: i64,
    quoted_status_id: Option<i64>,
    quoted_account_id: Option<i64>,
    state: i32,
    approval_uri: Option<&str>,
    legacy: bool,
) -> Result<i64, WriteError> {
    let quote_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO quotes (account_id, activity_uri, approval_uri, created_at, legacy, \
             quoted_account_id, quoted_status_id, state, status_id, updated_at) \
         VALUES ($1, NULL, $2, clock_timestamp(), $3, $4, $5, $6, $7, clock_timestamp()) \
         RETURNING id",
    )
    .bind(account_id)
    .bind(approval_uri)
    .bind(legacy)
    .bind(quoted_account_id)
    .bind(quoted_status_id)
    .bind(state)
    .bind(status_id)
    .fetch_one(&mut **transaction)
    .await?;
    if let Some(quoted_account_id) = quoted_account_id {
        sqlx::query(
            "INSERT INTO mentions (id, account_id, created_at, silent, status_id, updated_at) \
             VALUES (nextval('mentions_id_seq'), $1, clock_timestamp(), true, $2, clock_timestamp()) \
             ON CONFLICT (account_id, status_id) DO NOTHING",
        )
        .bind(quoted_account_id)
        .bind(status_id)
        .execute(&mut **transaction)
        .await?;
        if state == 1 {
            if !legacy {
                increment_quote_count(
                    transaction,
                    quoted_status_id.expect("accepted quote has a target"),
                )
                .await?;
            }
            record_outbox_in(
                transaction,
                &notification_job(quoted_account_id, NOTIFICATION_QUOTE, quote_id),
            )
            .await?;
        }
    }
    Ok(quote_id)
}

#[allow(clippy::too_many_lines)]
pub(super) async fn reconcile_remote_note_quote(
    transaction: &mut Transaction<'_, Postgres>,
    status_id: i64,
    account_id: i64,
    note: &RemoteNoteData,
    origin: &str,
) -> Result<bool, WriteError> {
    let existing_target_id = quote_target_id_for_status(transaction, status_id).await?;
    let Some(quote) = note.quote.as_ref() else {
        if let Some(existing_target_id) = existing_target_id {
            lock_statuses_in_order(transaction, &[existing_target_id]).await?;
        }
        let existing = locked_quote_for_status(transaction, status_id).await?;
        if let Some((quote_id, quoted_status_id, quoted_account_id, state, _, _, request_uri)) =
            existing
        {
            cancel_quote_request_outbox(transaction, quote_id, request_uri.as_deref()).await?;
            if state == 1
                && let (Some(quoted_status_id), Some(quoted_account_id)) =
                    (quoted_status_id, quoted_account_id)
                && sqlx::query_scalar::<_, bool>(
                    "SELECT domain IS NULL FROM accounts WHERE id = $1",
                )
                .bind(quoted_account_id)
                .fetch_one(&mut **transaction)
                .await?
            {
                record_quote_authorization_delete(
                    transaction,
                    quote_id,
                    status_id,
                    quoted_status_id,
                    quoted_account_id,
                    origin,
                )
                .await?;
            }
            if state == 1
                && let Some(quoted_status_id) = quoted_status_id
            {
                decrement_quote_count(transaction, quoted_status_id).await?;
            }
            if let Some(quoted_account_id) = quoted_account_id {
                delete_activity_notifications(transaction, quoted_account_id, quote_id, "Quote")
                    .await?;
                sqlx::query(
                    "DELETE FROM mentions WHERE status_id = $1 AND account_id = $2 AND silent = true",
                )
                .bind(status_id)
                .bind(quoted_account_id)
                .execute(&mut **transaction)
                .await?;
            }
            sqlx::query(
                "UPDATE quotes SET quoted_status_id = NULL, quoted_account_id = NULL, \
                        approval_uri = NULL, activity_uri = NULL, state = 4, legacy = true, \
                        updated_at = clock_timestamp() WHERE id = $1",
            )
            .bind(quote_id)
            .execute(&mut **transaction)
            .await?;
            return Ok(true);
        }
        return Ok(false);
    };
    let target = if quote.deleted {
        None
    } else {
        let target_uri = quote
            .target_uri
            .as_deref()
            .ok_or(WriteError::InvalidInput("remote quote has no target"))?;
        resolve_quote_target(transaction, target_uri, origin).await?
    };
    if target.is_none() && !quote.deleted {
        // There is no safe generic quote-target fetch path. Do not dereference an arbitrary URI.
        return Ok(false);
    }
    let mut target_status_ids = target
        .as_ref()
        .map(|(target_status_id, ..)| *target_status_id)
        .into_iter()
        .collect::<Vec<_>>();
    target_status_ids.extend(existing_target_id);
    lock_statuses_in_order(transaction, &target_status_ids).await?;
    let existing = locked_quote_for_status(transaction, status_id).await?;
    let (quoted_status_id, quoted_account_id, mut state, mut approval_uri) =
        if let Some((target_status_id, target_account_id, _, _)) = target {
            let approval_uri = None;
            let accepted = target_account_id == account_id;
            (
                Some(target_status_id),
                Some(target_account_id),
                i32::from(accepted),
                approval_uri,
            )
        } else {
            (None, None, 4, None)
        };
    if let Some((
        quote_id,
        old_target_id,
        old_account_id,
        old_state,
        old_approval,
        old_legacy,
        old_request_uri,
    )) = existing
    {
        let target_changed =
            old_target_id != quoted_status_id || old_account_id != quoted_account_id;
        let (reconciled_state, reconciled_approval_uri) = reconciled_remote_quote_state(
            target_changed,
            old_state,
            old_approval.as_deref(),
            quote.authorization_uri.as_deref(),
            state,
        );
        state = reconciled_state;
        approval_uri = reconciled_approval_uri;
        if old_target_id == quoted_status_id
            && old_account_id == quoted_account_id
            && old_state == state
            && old_approval == approval_uri
            && old_legacy == quote.legacy
        {
            return Ok(false);
        }
        cancel_quote_request_outbox(transaction, quote_id, old_request_uri.as_deref()).await?;
        if target_changed
            && old_state == 1
            && let (Some(old_target_id), Some(old_account_id)) = (old_target_id, old_account_id)
            && sqlx::query_scalar::<_, bool>("SELECT domain IS NULL FROM accounts WHERE id = $1")
                .bind(old_account_id)
                .fetch_one(&mut **transaction)
                .await?
        {
            record_quote_authorization_delete(
                transaction,
                quote_id,
                status_id,
                old_target_id,
                old_account_id,
                origin,
            )
            .await?;
        }
        if ((target_changed && old_state == 1)
            || (!target_changed
                && quote_state_update_counter_delta(quote.legacy, old_state, state) < 0))
            && let Some(old_target_id) = old_target_id
        {
            decrement_quote_count(transaction, old_target_id).await?;
        }
        if let Some(old_account_id) = old_account_id {
            if target_changed || old_state != state {
                delete_activity_notifications(transaction, old_account_id, quote_id, "Quote")
                    .await?;
            }
            if target_changed && Some(old_account_id) != quoted_account_id {
                sqlx::query(
                    "DELETE FROM mentions WHERE status_id = $1 AND account_id = $2 AND silent = true",
                )
                .bind(status_id)
                .bind(old_account_id)
                .execute(&mut **transaction)
                .await?;
            }
        }
        if target_changed {
            sqlx::query(
                "UPDATE quotes SET quoted_status_id = $2, quoted_account_id = $3, state = $4, \
                        approval_uri = $5, activity_uri = NULL, legacy = $6, \
                        updated_at = clock_timestamp() WHERE id = $1",
            )
            .bind(quote_id)
            .bind(quoted_status_id)
            .bind(quoted_account_id)
            .bind(state)
            .bind(&approval_uri)
            .bind(quote.legacy)
            .execute(&mut **transaction)
            .await?;
            if let Some(quoted_account_id) = quoted_account_id {
                sqlx::query(
                    "INSERT INTO mentions (id, account_id, created_at, silent, status_id, updated_at) \
                     VALUES (nextval('mentions_id_seq'), $1, clock_timestamp(), true, $2, clock_timestamp()) \
                     ON CONFLICT (account_id, status_id) DO NOTHING",
                )
                .bind(quoted_account_id)
                .bind(status_id)
                .execute(&mut **transaction)
                .await?;
                if state == 1 {
                    if !quote.legacy {
                        increment_quote_count(
                            transaction,
                            quoted_status_id.expect("accepted quote has a target"),
                        )
                        .await?;
                    }
                    record_outbox_in(
                        transaction,
                        &notification_job(quoted_account_id, NOTIFICATION_QUOTE, quote_id),
                    )
                    .await?;
                }
            }
            return Ok(true);
        }
        sqlx::query(
            "UPDATE quotes SET state = $2, approval_uri = $3, legacy = $4, \
                    updated_at = clock_timestamp() WHERE id = $1",
        )
        .bind(quote_id)
        .bind(state)
        .bind(&approval_uri)
        .bind(quote.legacy)
        .execute(&mut **transaction)
        .await?;
        if quote_state_update_counter_delta(quote.legacy, old_state, state) > 0
            && let Some(quoted_status_id) = quoted_status_id
        {
            increment_quote_count(transaction, quoted_status_id).await?;
            if let Some(quoted_account_id) = quoted_account_id {
                record_outbox_in(
                    transaction,
                    &notification_job(quoted_account_id, NOTIFICATION_QUOTE, quote_id),
                )
                .await?;
            }
        }
    } else {
        insert_reconciled_remote_quote(
            transaction,
            account_id,
            status_id,
            quoted_status_id,
            quoted_account_id,
            state,
            approval_uri.as_deref(),
            quote.legacy,
        )
        .await?;
    }
    Ok(true)
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn ensure_remote_note_conversation(
    transaction: &mut Transaction<'_, Postgres>,
    status_id: i64,
    account_id: i64,
    in_reply_to_id: Option<i64>,
    in_reply_to_account_id: Option<i64>,
    conversation_id: Option<i64>,
    conversation_uri: Option<&str>,
    created_at: NaiveDateTime,
) -> Result<Option<i64>, WriteError> {
    if conversation_id.is_some() {
        return Ok(conversation_id);
    }
    let conversation_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO conversations (
            created_at, parent_account_id, parent_status_id, updated_at, uri
         ) VALUES ($1, $2, $3, clock_timestamp(), $4)
         ON CONFLICT (uri) WHERE uri IS NOT NULL
         DO UPDATE SET updated_at = clock_timestamp()
         RETURNING id",
    )
    .bind(created_at)
    .bind(in_reply_to_account_id.or(Some(account_id)))
    .bind(in_reply_to_id.or(Some(status_id)))
    .bind(conversation_uri)
    .fetch_one(&mut **transaction)
    .await?;
    Ok(Some(conversation_id))
}

pub(super) async fn insert_remote_note(
    transaction: &mut Transaction<'_, Postgres>,
    account_id: i64,
    note: &RemoteNoteData,
    visibility: i32,
    in_reply_to_id: Option<i64>,
    in_reply_to_account_id: Option<i64>,
    conversation_id: Option<i64>,
) -> Result<i64, WriteError> {
    Ok(sqlx::query_scalar::<_, i64>(
        "INSERT INTO statuses (
            account_id, text, spoiler_text, visibility, local, language, sensitive, reply,
            uri, url, in_reply_to_id, in_reply_to_account_id, conversation_id,
            quote_approval_policy, created_at, updated_at, edited_at
         ) VALUES ($1, $2, $3, $4, false, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)
         RETURNING id",
    )
    .bind(account_id)
    .bind(&note.content)
    .bind(&note.summary)
    .bind(visibility)
    .bind(&note.language)
    .bind(note.sensitive)
    .bind(note.in_reply_to_uri.is_some() || in_reply_to_id.is_some())
    .bind(&note.uri)
    .bind(note.url.as_deref().or(Some(note.uri.as_str())))
    .bind(in_reply_to_id)
    .bind(in_reply_to_account_id)
    .bind(conversation_id)
    .bind(note.quote_approval_policy)
    .bind(note.published_at)
    .bind(note.updated_at)
    .bind(note.edited_at)
    .fetch_one(&mut **transaction)
    .await?)
}

#[allow(clippy::too_many_lines)]
pub(super) async fn finalize_poll_expiration_generation_in(
    transaction: &mut Transaction<'_, Postgres>,
    poll_id: i64,
    status_id: i64,
    owner_id: i64,
    owner_is_local: bool,
    expires_at: DateTime<Utc>,
    activation: DateTime<Utc>,
) -> Result<(), WriteError> {
    let generation = poll_expiration_generation(expires_at);
    if poll_expiration_effect_in(transaction, poll_id, generation)
        .await?
        .is_some()
    {
        return Ok(());
    }
    if poll_expiration_is_historical(expires_at, activation) {
        record_poll_expiration_effect_in(
            transaction,
            poll_id,
            generation,
            PollExpirationEffectOutcome::HistoricalBaseline,
        )
        .await?;
        return Ok(());
    }
    let now = sqlx::query_scalar::<_, DateTime<Utc>>("SELECT clock_timestamp()")
        .fetch_one(&mut **transaction)
        .await?;
    if expires_at > now {
        let expiration_job = poll_expiration_job(
            poll_id,
            expires_at,
            PollExpirationIntentKind::Reschedule,
            expires_at + ChronoDuration::minutes(5),
        );
        record_outbox_once_in(transaction, &expiration_job).await?;
        return Ok(());
    }
    sqlx::query(
        "WITH recipients AS ( \
            SELECT DISTINCT vote.account_id \
              FROM poll_votes vote \
              JOIN accounts voter ON voter.id = vote.account_id AND voter.domain IS NULL \
             WHERE vote.poll_id = $1 \
            UNION SELECT $2::bigint WHERE $3::boolean) \
         INSERT INTO rustodon.outbox_events (kind, logical_key, payload) \
         SELECT $4, format('notification:poll:%s:%s', account_id, $1), \
                jsonb_build_object( \
                    'lane', 'core', \
                    'arguments', jsonb_build_object( \
                        'recipient_account_id', account_id, \
                        'activity_type', 'poll', \
                        'activity_id', $1, \
                        'silenced', false), \
                    'run_at', $5::text, \
                    'max_attempts', 25) \
           FROM recipients \
         ON CONFLICT (kind, logical_key) WHERE logical_key IS NOT NULL DO NOTHING",
    )
    .bind(poll_id)
    .bind(owner_id)
    .bind(owner_is_local)
    .bind(NOTIFICATION_CREATE_JOB_KIND)
    .bind(now.to_rfc3339())
    .execute(&mut **transaction)
    .await?;
    if owner_is_local {
        let final_key = format!("activitypub:poll:{poll_id}:expired");
        let already_recorded = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM rustodon.outbox_events \
             WHERE kind = $1 AND logical_key = $2)",
        )
        .bind(ACTIVITYPUB_STATUS_DISTRIBUTION_JOB_KIND)
        .bind(&final_key)
        .fetch_one(&mut **transaction)
        .await?;
        if !already_recorded {
            let updated_at = sqlx::query_scalar::<_, NaiveDateTime>(
                "UPDATE polls SET lock_version = lock_version + 1, \
                 updated_at = clock_timestamp() WHERE id = $1 RETURNING updated_at",
            )
            .bind(poll_id)
            .fetch_one(&mut **transaction)
            .await?;
            let edited_at = status_federation_version(transaction, status_id).await?;
            let update_job = JobSpec::new(
                Lane::Push,
                ACTIVITYPUB_STATUS_DISTRIBUTION_JOB_KIND,
                json!({
                    "status_id": status_id,
                    "activity_type": "Update",
                    "update_kind": "poll",
                    "update_version_micros": updated_at.and_utc().timestamp_micros(),
                    "edited_at_micros": edited_at.and_utc().timestamp_micros(),
                    "poll_updated_at_micros": updated_at.and_utc().timestamp_micros()
                }),
            )
            .logical_key(final_key);
            record_outbox_once_in(transaction, &update_job).await?;
        }
    }
    record_poll_expiration_effect_in(
        transaction,
        poll_id,
        generation,
        PollExpirationEffectOutcome::EffectsEnqueued,
    )
    .await?;
    Ok(())
}

pub(super) async fn reconcile_remote_poll(
    transaction: &mut Transaction<'_, Postgres>,
    status_id: i64,
    account_id: i64,
    poll: Option<&RemotePollData>,
    allow_significant_changes: bool,
    reject_tally_regression: bool,
    mark_fetched: bool,
) -> Result<RemotePollReconcile, WriteError> {
    if let Some(poll) = poll {
        return upsert_remote_poll(
            transaction,
            status_id,
            account_id,
            poll,
            allow_significant_changes,
            reject_tally_regression,
            mark_fetched,
        )
        .await;
    }
    if !allow_significant_changes {
        return Ok(RemotePollReconcile::Unchanged);
    }
    let poll_ids = sqlx::query_scalar::<_, i64>(
        "SELECT id FROM polls WHERE status_id = $1 ORDER BY id FOR UPDATE",
    )
    .bind(status_id)
    .fetch_all(&mut **transaction)
    .await?;
    if poll_ids.is_empty() {
        return Ok(RemotePollReconcile::Unchanged);
    }
    sqlx::query("UPDATE statuses SET poll_id = NULL WHERE id = $1")
        .bind(status_id)
        .execute(&mut **transaction)
        .await?;
    sqlx::query("DELETE FROM notifications WHERE activity_type = 'Poll' AND activity_id = ANY($1)")
        .bind(&poll_ids)
        .execute(&mut **transaction)
        .await?;
    sqlx::query("DELETE FROM poll_votes WHERE poll_id = ANY($1)")
        .bind(&poll_ids)
        .execute(&mut **transaction)
        .await?;
    sqlx::query("DELETE FROM polls WHERE id = ANY($1)")
        .bind(&poll_ids)
        .execute(&mut **transaction)
        .await?;
    Ok(RemotePollReconcile::Significant)
}

pub(super) fn remote_poll_votes_count(poll: &RemotePollData) -> Result<i64, WriteError> {
    poll.tallies
        .iter()
        .try_fold(0_i64, |total, tally| total.checked_add(*tally))
        .ok_or(WriteError::InvalidInput(
            "remote poll vote count is invalid",
        ))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RemotePollExpirationChange {
    None,
    Reschedule,
    Suppress,
}

pub(super) fn remote_poll_previous_expiration_is_due_change(
    has_surviving_local_votes: bool,
    incoming: Option<NaiveDateTime>,
    previous: Option<NaiveDateTime>,
    database_now: NaiveDateTime,
) -> bool {
    has_surviving_local_votes
        && incoming != previous
        && previous.is_some_and(|expires_at| expires_at <= database_now)
}

pub(super) fn remote_poll_expiration_change(
    has_surviving_local_votes: bool,
    incoming: Option<NaiveDateTime>,
    previous: Option<NaiveDateTime>,
    database_now: NaiveDateTime,
) -> RemotePollExpirationChange {
    if !has_surviving_local_votes || incoming == previous {
        return RemotePollExpirationChange::None;
    }
    if previous.is_some_and(|expires_at| expires_at <= database_now) {
        return RemotePollExpirationChange::Suppress;
    }
    if incoming.is_some() {
        RemotePollExpirationChange::Reschedule
    } else {
        RemotePollExpirationChange::None
    }
}

#[allow(clippy::similar_names)]
pub(super) fn remote_poll_tallies_are_monotonic(
    cached_tallies: &[i64],
    cached_votes_count: i64,
    cached_voters_count: Option<i64>,
    poll: &RemotePollData,
    incoming_votes_count: i64,
) -> bool {
    cached_tallies.len() == poll.tallies.len()
        && cached_tallies
            .iter()
            .zip(&poll.tallies)
            .all(|(cached, incoming)| incoming >= cached)
        && incoming_votes_count >= cached_votes_count
        && match (cached_voters_count, poll.voters_count) {
            (Some(cached), Some(incoming)) => incoming >= cached,
            (Some(_), None) => false,
            _ => true,
        }
}

#[allow(clippy::similar_names, clippy::too_many_lines)]
pub(super) async fn upsert_remote_poll(
    transaction: &mut Transaction<'_, Postgres>,
    status_id: i64,
    account_id: i64,
    poll: &RemotePollData,
    allow_significant_changes: bool,
    reject_tally_regression: bool,
    mark_fetched: bool,
) -> Result<RemotePollReconcile, WriteError> {
    let existing = sqlx::query_as::<
        _,
        (
            i64,
            Vec<String>,
            Vec<i64>,
            i64,
            Option<i64>,
            bool,
            Option<NaiveDateTime>,
        ),
    >(
        "SELECT poll.id, poll.options, poll.cached_tallies, poll.votes_count,
                poll.voters_count, poll.multiple, poll.expires_at
           FROM polls poll WHERE poll.status_id = $1 FOR UPDATE",
    )
    .bind(status_id)
    .fetch_optional(&mut **transaction)
    .await?;
    let mut expiration_reschedule = None;
    let mut expiration_suppression = None;
    let mut expiration_activation = None;
    let (poll_id, outcome) = if let Some((
        poll_id,
        options,
        cached_tallies,
        votes_count,
        voters_count,
        multiple,
        previous_expiry,
    )) = existing
    {
        let shape_changed = options != poll.options || multiple != poll.multiple;
        if shape_changed && !allow_significant_changes {
            return Ok(RemotePollReconcile::Unchanged);
        }
        let incoming_votes_count = remote_poll_votes_count(poll)?;
        if reject_tally_regression
            && !shape_changed
            && !remote_poll_tallies_are_monotonic(
                &cached_tallies,
                votes_count,
                voters_count,
                poll,
                incoming_votes_count,
            )
        {
            return Ok(RemotePollReconcile::Unchanged);
        }
        if !shape_changed
            && cached_tallies == poll.tallies
            && votes_count == incoming_votes_count
            && voters_count == poll.voters_count
            && previous_expiry == poll.expires_at
        {
            if mark_fetched {
                // Signed refresh freshness is transport metadata, not a semantic poll version.
                sqlx::query("UPDATE polls SET last_fetched_at = clock_timestamp() WHERE id = $1")
                    .bind(poll_id)
                    .execute(&mut **transaction)
                    .await?;
            }
            return Ok(RemotePollReconcile::Unchanged);
        }
        let expiration_clock =
            sqlx::query_scalar::<_, NaiveDateTime>("SELECT clock_timestamp()::timestamp")
                .fetch_one(&mut **transaction)
                .await?;
        let has_local_votes = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS ( \
                SELECT 1 FROM poll_votes vote \
                JOIN accounts voter ON voter.id = vote.account_id \
                WHERE vote.poll_id = $1 AND voter.domain IS NULL)",
        )
        .bind(poll_id)
        .fetch_one(&mut **transaction)
        .await?;
        let has_surviving_local_votes = has_local_votes && !shape_changed;
        if remote_poll_previous_expiration_is_due_change(
            has_surviving_local_votes,
            poll.expires_at,
            previous_expiry,
            expiration_clock,
        ) {
            let activation = poll_expiration_activation_in(transaction).await?;
            expiration_activation = Some(activation);
            let previous_expiry = previous_expiry
                .expect("a due remote poll expiration change has a previous expiration")
                .and_utc();
            finalize_poll_expiration_generation_in(
                transaction,
                poll_id,
                status_id,
                account_id,
                false,
                previous_expiry,
                activation,
            )
            .await?;
        }
        match remote_poll_expiration_change(
            has_surviving_local_votes,
            poll.expires_at,
            previous_expiry,
            expiration_clock,
        ) {
            RemotePollExpirationChange::None => {}
            RemotePollExpirationChange::Reschedule => expiration_reschedule = poll.expires_at,
            RemotePollExpirationChange::Suppress => expiration_suppression = poll.expires_at,
        }
        if shape_changed {
            sqlx::query("DELETE FROM poll_votes WHERE poll_id = $1")
                .bind(poll_id)
                .execute(&mut **transaction)
                .await?;
        }
        let reset_tallies = vec![0_i64; poll.options.len()];
        let tallies = if shape_changed {
            reset_tallies.as_slice()
        } else {
            poll.tallies.as_slice()
        };
        let next_votes_count = if shape_changed {
            0
        } else {
            incoming_votes_count
        };
        let next_voters_count = if shape_changed {
            Some(0)
        } else {
            poll.voters_count
        };
        let updated_at = sqlx::query_scalar::<_, NaiveDateTime>(
            "UPDATE polls SET options = $2, cached_tallies = $3, votes_count = $4, \
                voters_count = $5, multiple = $6, expires_at = $7, \
                last_fetched_at = CASE WHEN $8 THEN clock_timestamp() ELSE last_fetched_at END, \
                lock_version = lock_version + 1, updated_at = clock_timestamp() WHERE id = $1 \
                RETURNING updated_at",
        )
        .bind(poll_id)
        .bind(&poll.options)
        .bind(tallies)
        .bind(next_votes_count)
        .bind(next_voters_count)
        .bind(poll.multiple)
        .bind(poll.expires_at)
        .bind(mark_fetched)
        .fetch_one(&mut **transaction)
        .await?;
        let outcome = if shape_changed {
            RemotePollReconcile::Significant
        } else {
            RemotePollReconcile::Tally(updated_at)
        };
        (poll_id, outcome)
    } else {
        if !allow_significant_changes {
            return Ok(RemotePollReconcile::Unchanged);
        }
        let poll_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO polls (account_id, status_id, options, cached_tallies, votes_count, \
                voters_count, multiple, hide_totals, expires_at, last_fetched_at, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, false, $8, \
                CASE WHEN $9 THEN clock_timestamp() END, clock_timestamp(), clock_timestamp()) \
             RETURNING id",
        )
        .bind(account_id)
        .bind(status_id)
        .bind(&poll.options)
        .bind(&poll.tallies)
        .bind(remote_poll_votes_count(poll)?)
        .bind(poll.voters_count)
        .bind(poll.multiple)
        .bind(poll.expires_at)
        .bind(mark_fetched)
        .fetch_one(&mut **transaction)
        .await?;
        (poll_id, RemotePollReconcile::Significant)
    };
    sqlx::query("UPDATE statuses SET poll_id = $2 WHERE id = $1")
        .bind(status_id)
        .bind(poll_id)
        .execute(&mut **transaction)
        .await?;
    if let Some(expires_at) = expiration_suppression {
        let expires_at = expires_at.and_utc();
        let generation = poll_expiration_generation(expires_at);
        let activation = if let Some(activation) = expiration_activation {
            activation
        } else {
            poll_expiration_activation_in(transaction).await?
        };
        let outcome = if poll_expiration_is_historical(expires_at, activation) {
            PollExpirationEffectOutcome::HistoricalBaseline
        } else {
            PollExpirationEffectOutcome::RemotePastExpirySuppressed
        };
        if poll_expiration_effect_in(transaction, poll_id, generation)
            .await?
            .is_none()
        {
            record_poll_expiration_effect_in(transaction, poll_id, generation, outcome).await?;
        }
    }
    if let Some(expires_at) = expiration_reschedule {
        let expires_at = expires_at.and_utc();
        let expiration_job = poll_expiration_job(
            poll_id,
            expires_at,
            PollExpirationIntentKind::Reschedule,
            expires_at + ChronoDuration::minutes(5),
        );
        record_outbox_once_in(transaction, &expiration_job).await?;
    }
    Ok(outcome)
}

pub(super) async fn insert_remote_note_media(
    transaction: &mut Transaction<'_, Postgres>,
    status_id: i64,
    account_id: i64,
    attachments: &[RemoteNoteAttachment],
) -> Result<Vec<i64>, WriteError> {
    let mut media_ids = Vec::new();
    for attachment in attachments.iter().take(4) {
        let media = sqlx::query_as::<_, (i64, Option<i32>, Option<String>)>(
            "UPDATE media_attachments SET
                type = CASE WHEN file_file_name IS NULL THEN $3 ELSE type END,
                description = $4,
                file_content_type = CASE WHEN file_file_name IS NULL THEN $6 ELSE file_content_type END,
                file_meta = CASE WHEN file_file_name IS NULL THEN $7::json
                    ELSE ((COALESCE(file_meta::jsonb, '{}'::jsonb) - 'focus') ||
                        CASE WHEN $7::jsonb ? 'focus' THEN jsonb_build_object('focus', $7::jsonb -> 'focus')
                             ELSE '{}'::jsonb END)::json END,
                blurhash = CASE WHEN file_file_name IS NULL THEN $8 ELSE blurhash END,
                thumbnail_remote_url = $9,
                processing = CASE WHEN file_file_name IS NULL THEN $10 ELSE processing END,
                updated_at = clock_timestamp()
              WHERE account_id = $1 AND status_id = $2 AND remote_url = $5
              RETURNING id, processing, file_file_name",
        )
        .bind(account_id)
        .bind(status_id)
        .bind(remote_media_type(attachment.content_type.as_deref()))
        .bind(&attachment.description)
        .bind(&attachment.remote_url)
        .bind(&attachment.content_type)
        .bind(&attachment.file_meta)
        .bind(&attachment.blurhash)
        .bind(&attachment.thumbnail_remote_url)
        .bind(remote_media_processing(attachment.content_type.as_deref()))
        .fetch_optional(&mut **transaction)
        .await?;
        let (media_id, processing, file_file_name) = if let Some(media) = media {
            media
        } else {
            let media_id = sqlx::query_scalar::<_, i64>(
                "INSERT INTO media_attachments (
                account_id, status_id, type, processing, description, remote_url,
                file_content_type, file_meta, blurhash, thumbnail_remote_url,
                created_at, updated_at
             ) VALUES ($1, $2, $3, $10, $4, $5, $6, $7::json, $8, $9,
                       clock_timestamp(), clock_timestamp())
             RETURNING id",
            )
            .bind(account_id)
            .bind(status_id)
            .bind(remote_media_type(attachment.content_type.as_deref()))
            .bind(&attachment.description)
            .bind(&attachment.remote_url)
            .bind(&attachment.content_type)
            .bind(&attachment.file_meta)
            .bind(&attachment.blurhash)
            .bind(&attachment.thumbnail_remote_url)
            .bind(remote_media_processing(attachment.content_type.as_deref()))
            .fetch_one(&mut **transaction)
            .await?;
            (
                media_id,
                Some(remote_media_processing(attachment.content_type.as_deref())),
                None,
            )
        };
        if file_file_name.is_none() && remote_media_is_fetchable(attachment.content_type.as_deref())
        {
            let media_job = JobSpec::new(
                Lane::Pull,
                ACTIVITYPUB_MEDIA_FETCH_JOB_KIND,
                json!({"media_id": media_id}),
            )
            .logical_key(remote_media_job_logical_key(
                media_id,
                &attachment.remote_url,
            ))
            .max_attempts(4);
            if processing == Some(3) {
                record_outbox_in(transaction, &media_job).await?;
            } else {
                record_outbox_once_in(transaction, &media_job).await?;
            }
        }
        media_ids.push(media_id);
    }
    Ok(media_ids)
}

pub(super) fn remote_media_processing(content_type: Option<&str>) -> i32 {
    if remote_media_is_fetchable(content_type) {
        0
    } else {
        2
    }
}

pub(super) fn remote_media_is_fetchable(content_type: Option<&str>) -> bool {
    // Missing advertisement is allowed only through the bounded response-MIME policy.
    crate::media::RemoteMediaPolicy::new(content_type).is_some()
}

pub(super) fn remote_media_job_logical_key(media_id: i64, remote_url: &str) -> String {
    let digest = Sha256::digest(remote_url.as_bytes());
    let mut digest_string = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        write!(&mut digest_string, "{byte:02x}").expect("writing to a String cannot fail");
    }
    format!("activitypub:media:{media_id}:{digest_string}")
}

pub(super) async fn remove_remote_note_media_not_in(
    transaction: &mut Transaction<'_, Postgres>,
    status_id: i64,
    attachments: &[RemoteNoteAttachment],
) -> Result<(), WriteError> {
    let remote_urls = attachments
        .iter()
        .take(4)
        .map(|attachment| attachment.remote_url.clone())
        .collect::<Vec<_>>();
    if remote_urls.is_empty() {
        sqlx::query("DELETE FROM media_attachments WHERE status_id = $1")
            .bind(status_id)
            .execute(&mut **transaction)
            .await?;
    } else {
        sqlx::query(
            "DELETE FROM media_attachments
              WHERE status_id = $1 AND NOT (remote_url = ANY($2))",
        )
        .bind(status_id)
        .bind(remote_urls)
        .execute(&mut **transaction)
        .await?;
    }
    Ok(())
}

pub(super) async fn insert_remote_note_mentions(
    transaction: &mut Transaction<'_, Postgres>,
    status_id: i64,
    note: &RemoteNoteData,
    delivery_target_account_id: Option<i64>,
    origin: &str,
) -> Result<Vec<(i64, i64)>, WriteError> {
    let mut targets = HashMap::new();
    for uri in &note.mentions {
        targets.insert(uri.as_str(), false);
    }
    for uri in note.audience.to.iter().chain(&note.audience.cc) {
        targets.entry(uri.as_str()).or_insert(true);
    }
    let mut mention_ids = Vec::new();
    for (uri, silent) in targets {
        let Some(account_id) = local_activitypub_account_id(transaction, uri, origin).await? else {
            continue;
        };
        let mention_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO mentions (account_id, status_id, silent, created_at, updated_at)
             VALUES ($1, $2, $3, clock_timestamp(), clock_timestamp())
             ON CONFLICT (account_id, status_id) DO UPDATE SET silent = mentions.silent AND $3
             RETURNING id",
        )
        .bind(account_id)
        .bind(status_id)
        .bind(silent)
        .fetch_one(&mut **transaction)
        .await?;
        if !silent {
            mention_ids.push((mention_id, account_id));
        }
    }
    if let Some(account_id) = delivery_target_account_id {
        sqlx::query(
            "INSERT INTO mentions (account_id, status_id, silent, created_at, updated_at)
             SELECT $1, $2, true, clock_timestamp(), clock_timestamp()
             WHERE EXISTS (SELECT 1 FROM accounts WHERE id = $1 AND domain IS NULL)
             ON CONFLICT (account_id, status_id) DO NOTHING",
        )
        .bind(account_id)
        .bind(status_id)
        .execute(&mut **transaction)
        .await?;
    }
    Ok(mention_ids)
}

pub(super) async fn ensure_remote_note_delivery_target(
    transaction: &mut Transaction<'_, Postgres>,
    status_id: i64,
    account_id: i64,
) -> Result<(), WriteError> {
    sqlx::query(
        "INSERT INTO mentions (account_id, status_id, silent, created_at, updated_at)
         SELECT $1, $2, true, clock_timestamp(), clock_timestamp()
         WHERE EXISTS (SELECT 1 FROM accounts WHERE id = $1 AND domain IS NULL)
         ON CONFLICT (account_id, status_id) DO NOTHING",
    )
    .bind(account_id)
    .bind(status_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

pub(super) async fn update_remote_note_tags(
    transaction: &mut Transaction<'_, Postgres>,
    status_id: i64,
    hashtags: &[String],
) -> Result<(), WriteError> {
    for hashtag in hashtags {
        let tag_id = sqlx::query_scalar::<_, i64>(
            "SELECT id FROM tags WHERE lower(name) = lower($1) LIMIT 1",
        )
        .bind(hashtag)
        .fetch_optional(&mut **transaction)
        .await?
        .or(sqlx::query_scalar::<_, i64>(
            "INSERT INTO tags (name, display_name, created_at, updated_at)
                 VALUES ($1, $1, clock_timestamp(), clock_timestamp())
                 ON CONFLICT DO NOTHING RETURNING id",
        )
        .bind(hashtag)
        .fetch_optional(&mut **transaction)
        .await?)
        .or(sqlx::query_scalar::<_, i64>(
            "SELECT id FROM tags WHERE lower(name) = lower($1) LIMIT 1",
        )
        .bind(hashtag)
        .fetch_optional(&mut **transaction)
        .await?);
        let Some(tag_id) = tag_id else {
            continue;
        };
        sqlx::query(
            "INSERT INTO statuses_tags (status_id, tag_id) VALUES ($1, $2)
             ON CONFLICT DO NOTHING",
        )
        .bind(status_id)
        .bind(tag_id)
        .execute(&mut **transaction)
        .await?;
    }
    Ok(())
}

pub(super) async fn insert_remote_note_stats(
    transaction: &mut Transaction<'_, Postgres>,
    status_id: i64,
    note: &RemoteNoteData,
) -> Result<(), WriteError> {
    sqlx::query(
        "INSERT INTO status_stats (
            status_id, untrusted_favourites_count, untrusted_reblogs_count,
            created_at, updated_at
         ) VALUES ($1, $2, $3, clock_timestamp(), clock_timestamp())",
    )
    .bind(status_id)
    .bind(note.favourites_count)
    .bind(note.reblogs_count)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

pub(super) async fn update_remote_note_stats(
    transaction: &mut Transaction<'_, Postgres>,
    status_id: i64,
    note: &RemoteNoteData,
) -> Result<(), WriteError> {
    sqlx::query(
        "UPDATE status_stats SET untrusted_favourites_count = COALESCE($2, untrusted_favourites_count),
            untrusted_reblogs_count = COALESCE($3, untrusted_reblogs_count),
            updated_at = clock_timestamp()
          WHERE status_id = $1",
    )
    .bind(status_id)
    .bind(note.favourites_count)
    .bind(note.reblogs_count)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

pub(super) fn remote_note_uri(value: Option<&Value>) -> Result<Option<String>, WriteError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = match value {
        Value::String(value) => value.clone(),
        Value::Object(object) => object
            .get("id")
            .or_else(|| object.get("href"))
            .and_then(Value::as_str)
            .ok_or(WriteError::InvalidInput("remote URI object has no ID"))?
            .to_owned(),
        _ => return Err(WriteError::InvalidInput("remote URI is invalid")),
    };
    let url = Url::parse(&value).map_err(|_| WriteError::InvalidInput("remote URI is invalid"))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(WriteError::InvalidInput("remote URI is invalid"));
    }
    Ok(Some(value))
}

pub(super) fn remote_note_optional_uri(
    value: Option<&Value>,
) -> Result<Option<String>, WriteError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(value) => remote_note_uri(Some(value)),
    }
}

pub(super) fn remote_note_optional_conversation_uri(
    value: Option<&Value>,
) -> Result<Option<String>, WriteError> {
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let uri = match value {
        Value::String(value) => value.clone(),
        Value::Object(object) => object
            .get("id")
            .or_else(|| object.get("href"))
            .and_then(Value::as_str)
            .ok_or(WriteError::InvalidInput(
                "remote conversation URI is invalid",
            ))?
            .to_owned(),
        _ => {
            return Err(WriteError::InvalidInput(
                "remote conversation URI is invalid",
            ));
        }
    };
    if let Some(tag_uri) = uri.strip_prefix("tag:") {
        if tag_uri.trim().is_empty() {
            return Err(WriteError::InvalidInput(
                "remote conversation URI is invalid",
            ));
        }
        return Ok(Some(uri));
    }
    remote_note_uri(Some(&Value::String(uri)))
}

pub(super) fn remote_note_attributed_to(
    value: Option<&Value>,
) -> Result<Option<String>, WriteError> {
    match value {
        Some(Value::Array(values)) => remote_note_uri(values.first()),
        _ => remote_note_uri(value),
    }
}

pub(super) fn remote_note_uri_array(value: Option<&Value>) -> Result<Vec<String>, WriteError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    if value.is_null() {
        return Ok(Vec::new());
    }
    let values = match value {
        Value::Array(values) => values.iter().collect::<Vec<_>>(),
        Value::String(_) | Value::Object(_) => vec![value],
        _ => return Err(WriteError::InvalidInput("remote Note audience is invalid")),
    };
    if values.len() > 100 {
        return Err(WriteError::InvalidInput(
            "remote Note audience is too large",
        ));
    }
    values
        .into_iter()
        .map(|value| {
            if value.as_str().is_some_and(activitypub::is_public_address) {
                return Ok(value.as_str().unwrap_or_default().to_owned());
            }
            remote_note_uri(Some(value)).and_then(|uri| {
                uri.ok_or(WriteError::InvalidInput(
                    "remote Note audience URI is invalid",
                ))
            })
        })
        .collect()
}

pub(super) fn remote_note_timestamp(
    object: &serde_json::Map<String, Value>,
    field: &str,
    fallback: NaiveDateTime,
) -> Result<NaiveDateTime, WriteError> {
    let Some(value) = object.get(field) else {
        return Ok(fallback);
    };
    let value = value
        .as_str()
        .ok_or(WriteError::InvalidInput("remote Note timestamp is invalid"))?;
    let timestamp = DateTime::parse_from_rfc3339(value)
        .map(|timestamp| timestamp.naive_utc())
        .map_err(|_| WriteError::InvalidInput("remote Note timestamp is invalid"))?;
    if timestamp > Utc::now().naive_utc() + ChronoDuration::hours(24) {
        return Err(WriteError::InvalidInput(
            "remote Note timestamp is too far in the future",
        ));
    }
    Ok(timestamp)
}

pub(super) const MAX_REMOTE_NOTE_COUNT: i64 = 100_000_000;
pub(super) const MAX_REMOTE_NOTE_COUNT_U64: u64 = 100_000_000;

pub(super) fn remote_note_count(
    object: &serde_json::Map<String, Value>,
    field: &str,
) -> Result<Option<i64>, WriteError> {
    let Some(value) = object.get(field) else {
        return Ok(None);
    };
    let count = value
        .as_i64()
        .map(|count| count.clamp(0, MAX_REMOTE_NOTE_COUNT))
        .or_else(|| {
            value
                .as_u64()
                .and_then(|count| i64::try_from(count.min(MAX_REMOTE_NOTE_COUNT_U64)).ok())
        })
        .ok_or(WriteError::InvalidInput("remote Note count is invalid"))?;
    Ok(Some(count))
}

pub(super) fn remote_note_interaction_count(
    object: &serde_json::Map<String, Value>,
    collection_field: &str,
    legacy_field: &str,
) -> Result<Option<i64>, WriteError> {
    if let Some(collection) = object.get(collection_field) {
        let Some(collection) = collection.as_object() else {
            return Ok(None);
        };
        return remote_note_count(collection, "totalItems");
    }
    remote_note_count(object, legacy_field)
}

pub(super) fn remote_note_tags(
    value: Option<&Value>,
) -> Result<(Vec<String>, Vec<String>), WriteError> {
    let Some(value) = value else {
        return Ok((Vec::new(), Vec::new()));
    };
    let values = match value {
        Value::Array(values) => values.iter().collect::<Vec<_>>(),
        Value::Object(_) | Value::String(_) => vec![value],
        _ => return Err(WriteError::InvalidInput("remote Note tags are invalid")),
    };
    if values.len() > 100 {
        return Err(WriteError::InvalidInput("remote Note tags are too large"));
    }
    let mut mentions = Vec::new();
    let mut hashtags = Vec::new();
    for value in values {
        let Some(object) = value.as_object() else {
            continue;
        };
        match object.get("type").and_then(Value::as_str) {
            Some("Mention") => {
                if let Some(uri) = remote_note_optional_uri(object.get("href"))? {
                    mentions.push(uri);
                }
            }
            Some("Hashtag") => {
                if let Some(name) = object.get("name").and_then(Value::as_str) {
                    let name = name.trim_start_matches('#').trim().to_ascii_lowercase();
                    if !name.is_empty() && name.chars().count() <= 100 && !hashtags.contains(&name)
                    {
                        hashtags.push(name);
                    }
                }
            }
            _ => {}
        }
    }
    Ok((mentions, hashtags))
}

pub(super) fn remote_note_attachments(value: Option<&Value>) -> Vec<RemoteNoteAttachment> {
    let Some(value) = value else {
        return Vec::new();
    };
    let values = match value {
        Value::Array(values) => values.iter().collect::<Vec<_>>(),
        Value::Object(_) => vec![value],
        _ => return Vec::new(),
    };
    let mut attachments = Vec::new();
    for value in values {
        if attachments.len() == 4 {
            break;
        }
        let Some(object) = value.as_object() else {
            continue;
        };
        let Some(remote_url) = remote_attachment_url(object.get("url")) else {
            continue;
        };
        let thumbnail_remote_url =
            remote_attachment_url(object.get("icon").or_else(|| object.get("preview")));
        let content_type = object
            .get("mediaType")
            .and_then(Value::as_str)
            .or_else(|| remote_attachment_media_type(object.get("url")))
            .filter(|value| value.chars().count() <= 255)
            .map(ToOwned::to_owned);
        let description = object
            .get("summary")
            .and_then(Value::as_str)
            .or_else(|| object.get("name").and_then(Value::as_str))
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.chars().take(10_000).collect());
        let blurhash = remote_attachment_blurhash(object.get("blurhash"));
        let mut file_meta = Map::new();
        for field in ["width", "height"] {
            if let Some(value) = object.get(field).and_then(Value::as_i64)
                && value >= 0
            {
                file_meta.insert(field.to_owned(), json!(value));
            }
        }
        if let Some(Value::Array(values)) = object.get("focalPoint")
            && values.len() == 2
            && let (Some(x), Some(y)) = (values[0].as_f64(), values[1].as_f64())
            && x.is_finite()
            && y.is_finite()
        {
            file_meta.insert("focus".to_owned(), json!({"x": x, "y": y}));
        }
        attachments.push(RemoteNoteAttachment {
            remote_url,
            thumbnail_remote_url,
            content_type,
            description,
            blurhash,
            file_meta: Value::Object(file_meta),
        });
    }
    attachments
}

pub(super) fn remote_attachment_url(value: Option<&Value>) -> Option<String> {
    match value {
        Some(Value::String(value)) => remote_attachment_uri(value),
        Some(Value::Object(object)) => object
            .get("href")
            .or_else(|| object.get("url"))
            .or_else(|| object.get("id"))
            .and_then(|value| remote_attachment_url(Some(value))),
        Some(Value::Array(values)) => values
            .iter()
            .find_map(|value| remote_attachment_url(Some(value))),
        _ => None,
    }
}

pub(super) fn remote_attachment_media_type(value: Option<&Value>) -> Option<&str> {
    match value {
        Some(Value::Object(object)) => object.get("mediaType").and_then(Value::as_str),
        Some(Value::Array(values)) => values
            .iter()
            .find_map(|value| remote_attachment_media_type(Some(value))),
        _ => None,
    }
}

pub(super) fn remote_attachment_uri(value: &str) -> Option<String> {
    let url = Url::parse(value).ok()?;
    if matches!(url.scheme(), "http" | "https") && url.host_str().is_some() {
        Some(url.to_string())
    } else {
        None
    }
}

pub(super) fn remote_attachment_blurhash(value: Option<&Value>) -> Option<String> {
    let value = value.and_then(Value::as_str)?;
    if value.len() > 255 || !value.is_ascii() || value.len() < 6 {
        return None;
    }
    let alphabet =
        b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz#$%*+-.:;=?@[]^_{|}~";
    let size_flag = alphabet
        .iter()
        .position(|character| *character == value.as_bytes()[0])?;
    let components_x = size_flag % 9 + 1;
    let components_y = size_flag / 9 + 1;
    if components_x > 5
        || components_y > 5
        || value.len() != 4 + 2 * components_x * components_y
        || blurhash::decode(value, 1, 1, 1.0).is_err()
    {
        return None;
    }
    Some(value.to_owned())
}

pub(super) fn remote_media_type(content_type: Option<&str>) -> i32 {
    match content_type {
        Some(value) if value.eq_ignore_ascii_case("image/gif") => 1,
        Some(value)
            if value
                .split(';')
                .next()
                .is_some_and(|value| value.trim().to_ascii_lowercase().starts_with("video/")) =>
        {
            2
        }
        Some(value)
            if value
                .split(';')
                .next()
                .is_some_and(|value| value.trim().to_ascii_lowercase().starts_with("audio/")) =>
        {
            4
        }
        _ => 0,
    }
}
