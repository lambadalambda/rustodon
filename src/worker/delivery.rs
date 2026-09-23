//! Outbound delivery: currency checks for updates, deletes and quotes, and failure classification.

use super::*;

pub(super) fn update_delivery_is_current(
    activity_id: Option<&str>,
    object_uri: &str,
    version: NaiveDateTime,
    requested_version_micros: Option<i64>,
) -> bool {
    if requested_version_micros
        .is_some_and(|requested| requested != version.and_utc().timestamp_micros())
    {
        return false;
    }
    let expected_activity_id = activitypub::update_activity_id(object_uri, version);
    // Already-persisted deliveries keep their original body/ID across an upgrade.
    // Their microsecond metadata still fences superseded versions.
    let legacy_activity_id = format!("{object_uri}#updates/{}", version.and_utc().timestamp());
    activity_id == Some(expected_activity_id.as_str())
        || activity_id == Some(legacy_activity_id.as_str())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn status_update_delivery_is_current(
    update_kind: StatusUpdateKind,
    activity_id: Option<&str>,
    object_uri: &str,
    current_edited_at: NaiveDateTime,
    current_poll_updated_at: Option<NaiveDateTime>,
    requested_edited_at_micros: Option<i64>,
    requested_poll_updated_at_micros: Option<i64>,
    requested_update_version_micros: Option<i64>,
) -> bool {
    if requested_edited_at_micros != Some(current_edited_at.and_utc().timestamp_micros())
        || requested_poll_updated_at_micros
            != current_poll_updated_at.map(|value| value.and_utc().timestamp_micros())
    {
        return false;
    }
    let expected_update_version = match update_kind {
        StatusUpdateKind::Status
        | StatusUpdateKind::InteractionPolicy
        | StatusUpdateKind::StatusRepair => current_edited_at,
        StatusUpdateKind::Poll => {
            let Some(poll_updated_at) = current_poll_updated_at else {
                return false;
            };
            poll_updated_at
        }
        StatusUpdateKind::PollRepair => {
            let Some(poll_updated_at) = current_poll_updated_at else {
                return false;
            };
            current_edited_at.max(poll_updated_at)
        }
        StatusUpdateKind::Quote => {
            let Some(version_micros) = requested_update_version_micros else {
                return false;
            };
            let Some(version) = DateTime::<Utc>::from_timestamp_micros(version_micros) else {
                return false;
            };
            version.naive_utc()
        }
    };
    if requested_update_version_micros != Some(expected_update_version.and_utc().timestamp_micros())
    {
        return false;
    }
    if update_kind.is_repair() {
        return activity_id
            == Some(
                status_snapshot_repair_activity_id(
                    object_uri,
                    current_edited_at,
                    current_poll_updated_at,
                )
                .as_str(),
            );
    }
    update_delivery_is_current(
        activity_id,
        object_uri,
        expected_update_version,
        requested_update_version_micros,
    )
}

pub(super) fn complete_status_update_delivery_kind(
    update_kind: Option<&str>,
    has_current_poll: bool,
    requested_edited_at_micros: Option<i64>,
    requested_poll_updated_at_micros: Option<i64>,
    requested_update_version_micros: Option<i64>,
    published_version_micros: Option<i64>,
) -> Option<StatusUpdateKind> {
    let update_kind = StatusUpdateKind::parse(update_kind?)?;
    let edited_at_micros = requested_edited_at_micros?;
    if requested_poll_updated_at_micros.is_some() != has_current_poll
        || (update_kind.has_poll_reach() && !has_current_poll)
        || (update_kind == StatusUpdateKind::StatusRepair && has_current_poll)
    {
        return None;
    }
    let expected_update_version_micros = match update_kind {
        StatusUpdateKind::Status
        | StatusUpdateKind::InteractionPolicy
        | StatusUpdateKind::StatusRepair => edited_at_micros,
        StatusUpdateKind::Poll => requested_poll_updated_at_micros?,
        StatusUpdateKind::PollRepair => edited_at_micros.max(requested_poll_updated_at_micros?),
        StatusUpdateKind::Quote => requested_update_version_micros?,
    };
    if requested_update_version_micros != Some(expected_update_version_micros)
        || published_version_micros != Some(expected_update_version_micros)
    {
        return None;
    }
    Some(update_kind)
}

pub(super) fn inferred_current_repair_delivery_kind(
    activity_id: Option<&str>,
    object_uri: &str,
    current_edited_at: NaiveDateTime,
    current_poll_updated_at: Option<NaiveDateTime>,
    published_version_micros: Option<i64>,
) -> Option<StatusUpdateKind> {
    let update_kind = if current_poll_updated_at.is_some() {
        StatusUpdateKind::PollRepair
    } else {
        StatusUpdateKind::StatusRepair
    };
    let expected_update_version = current_poll_updated_at
        .map_or(current_edited_at, |poll_updated_at| {
            current_edited_at.max(poll_updated_at)
        });
    let expected_activity_id =
        status_snapshot_repair_activity_id(object_uri, current_edited_at, current_poll_updated_at);
    (activity_id == Some(expected_activity_id.as_str())
        && published_version_micros == Some(expected_update_version.and_utc().timestamp_micros()))
    .then_some(update_kind)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum QuoteDeliveryKind {
    Request,
    Accept,
    Reject,
}

#[derive(Debug, Eq, PartialEq)]
pub(super) struct QuoteDeliveryIdentity {
    pub(super) kind: QuoteDeliveryKind,
    pub(super) request_uri: String,
    pub(super) quote_id: Option<i64>,
    pub(super) quoting_status_id: Option<i64>,
    pub(super) quoted_status_id: i64,
}

pub(super) fn quote_delivery_identity(
    arguments: &Value,
    body: &Value,
) -> Result<Option<QuoteDeliveryIdentity>, ()> {
    let body_type = body.get("type").and_then(Value::as_str);
    let nested_type = body
        .get("object")
        .and_then(|object| object.get("type"))
        .and_then(Value::as_str);
    let body_kind = match (body_type, nested_type) {
        (Some("QuoteRequest"), _) => Some(QuoteDeliveryKind::Request),
        (Some("Accept"), Some("QuoteRequest")) => Some(QuoteDeliveryKind::Accept),
        (Some("Reject"), Some("QuoteRequest")) => Some(QuoteDeliveryKind::Reject),
        _ => None,
    };
    let metadata_kind = match arguments.get("quote_delivery_kind").and_then(Value::as_str) {
        Some("request") => Some(QuoteDeliveryKind::Request),
        Some("accept") => Some(QuoteDeliveryKind::Accept),
        Some("reject") => Some(QuoteDeliveryKind::Reject),
        Some(_) => return Err(()),
        None => None,
    };
    let Some(kind) = body_kind else {
        return if metadata_kind.is_some() {
            Err(())
        } else {
            Ok(None)
        };
    };
    if metadata_kind != Some(kind) {
        return Err(());
    }
    let request_uri = arguments
        .get("quote_request_uri")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or(())?;
    let body_request_uri = match kind {
        QuoteDeliveryKind::Request => body.get("id").and_then(Value::as_str),
        QuoteDeliveryKind::Accept | QuoteDeliveryKind::Reject => body
            .get("object")
            .and_then(|object| object.get("id"))
            .and_then(Value::as_str),
    };
    if body_request_uri != Some(request_uri)
        || (kind == QuoteDeliveryKind::Accept && body.get("result").is_none_or(Value::is_null))
        || (kind == QuoteDeliveryKind::Reject
            && body.get("result").is_some_and(|value| !value.is_null()))
    {
        return Err(());
    }
    let quote_id = arguments.get("quote_id").and_then(Value::as_i64);
    let quoting_status_id = arguments.get("quoting_status_id").and_then(Value::as_i64);
    let quoted_status_id = arguments
        .get("quoted_status_id")
        .and_then(Value::as_i64)
        .ok_or(())?;
    match kind {
        QuoteDeliveryKind::Request | QuoteDeliveryKind::Accept
            if quote_id.is_none() || quoting_status_id.is_none() =>
        {
            return Err(());
        }
        QuoteDeliveryKind::Reject if quote_id.is_some() != quoting_status_id.is_some() => {
            return Err(());
        }
        _ => {}
    }
    Ok(Some(QuoteDeliveryIdentity {
        kind,
        request_uri: request_uri.to_owned(),
        quote_id,
        quoting_status_id,
        quoted_status_id,
    }))
}

pub(super) fn quote_body_uri(value: Option<&Value>) -> Option<&str> {
    remote_uri_value(value).filter(|value| !value.is_empty())
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(super) async fn quote_delivery_is_current_and_locked(
    transaction: &mut Transaction<'_, Postgres>,
    identity: &QuoteDeliveryIdentity,
    body: &Value,
    source_account_id: i64,
    source_actor_uri: &str,
    inbox_url: &str,
    configured_remote_domain: Option<&str>,
    origin: &str,
) -> Result<bool, sqlx::Error> {
    if quote_body_uri(body.get("actor")) != Some(source_actor_uri) {
        return Ok(false);
    }
    let request = if identity.kind == QuoteDeliveryKind::Request {
        body
    } else {
        body.get("object").unwrap_or(&Value::Null)
    };
    let Some(request_actor_uri) = quote_body_uri(request.get("actor")) else {
        return Ok(false);
    };
    let Some(request_target_uri) = quote_body_uri(request.get("object")) else {
        return Ok(false);
    };
    let Some(request_instrument_uri) = quote_body_uri(request.get("instrument")) else {
        return Ok(false);
    };
    let mut status_ids = vec![identity.quoted_status_id];
    if let Some(quoting_status_id) = identity.quoting_status_id {
        status_ids.push(quoting_status_id);
    }
    status_ids.sort_unstable();
    status_ids.dedup();
    let mut account_ids = sqlx::query_scalar::<_, i64>(
        "SELECT DISTINCT account_id FROM statuses WHERE id = ANY($1::bigint[]) ORDER BY account_id",
    )
    .bind(&status_ids)
    .fetch_all(&mut **transaction)
    .await?;
    account_ids.push(source_account_id);
    let requester_id = sqlx::query_scalar::<_, i64>(
        "SELECT id FROM accounts WHERE uri = $1 AND domain IS NOT NULL ORDER BY id LIMIT 1",
    )
    .bind(request_actor_uri)
    .fetch_optional(&mut **transaction)
    .await?;
    if let Some(requester_id) = requester_id {
        account_ids.push(requester_id);
    }
    let peer_account_id = if identity.kind == QuoteDeliveryKind::Request {
        sqlx::query_scalar::<_, i64>("SELECT account_id FROM statuses WHERE id = $1")
            .bind(identity.quoted_status_id)
            .fetch_optional(&mut **transaction)
            .await?
    } else {
        requester_id
    };
    let Some(peer_account_id) = peer_account_id.filter(|id| *id != source_account_id) else {
        return Ok(false);
    };
    sqlx::query(
        "SELECT pg_advisory_xact_lock( \
           hashtextextended(LEAST($1, $2)::text || ':' || GREATEST($1, $2)::text, 0))",
    )
    .bind(source_account_id)
    .bind(peer_account_id)
    .execute(&mut **transaction)
    .await?;
    account_ids.sort_unstable();
    account_ids.dedup();
    sqlx::query_scalar::<_, i64>(
        "SELECT id FROM accounts WHERE id = ANY($1::bigint[]) ORDER BY id FOR SHARE",
    )
    .bind(&account_ids)
    .fetch_all(&mut **transaction)
    .await?;
    let locked_status_ids = sqlx::query_scalar::<_, i64>(
        "SELECT id FROM statuses WHERE id = ANY($1::bigint[]) ORDER BY id FOR SHARE",
    )
    .bind(&status_ids)
    .fetch_all(&mut **transaction)
    .await?;
    if locked_status_ids.len() != status_ids.len() {
        return Ok(false);
    }
    if sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS ( \
           SELECT 1 FROM blocks \
           WHERE (account_id = $1 AND target_account_id = $2) \
              OR (account_id = $2 AND target_account_id = $1))",
    )
    .bind(source_account_id)
    .bind(peer_account_id)
    .fetch_one(&mut **transaction)
    .await?
    {
        return Ok(false);
    }
    let origin = origin.trim_end_matches('/');
    match identity.kind {
        QuoteDeliveryKind::Request => {
            let Some(quote_id) = identity.quote_id else {
                return Ok(false);
            };
            let Some(quoting_status_id) = identity.quoting_status_id else {
                return Ok(false);
            };
            let row = sqlx::query_as::<_, (bool, bool, String, String, String, String)>(
                "SELECT \
                   ($7 = quoting.uri OR $7 = quoting.url OR \
                    $7 = $10 || '/actor/statuses/' || quoting.id::text OR \
                    $7 = $10 || '/@' || source.username || '/' || quoting.id::text OR \
                    $7 = $10 || '/users/' || source.username || '/statuses/' || quoting.id::text OR \
                    $7 = $10 || '/ap/users/' || source.id::text || '/statuses/' || quoting.id::text), \
                   ($8 = quoted.uri OR $8 = quoted.url), target.uri, target.inbox_url, \
                   target.shared_inbox_url, target.domain \
                 FROM quotes quote \
                 JOIN statuses quoting ON quoting.id = quote.status_id \
                 JOIN accounts source ON source.id = quoting.account_id \
                 JOIN statuses quoted ON quoted.id = quote.quoted_status_id \
                 JOIN accounts target ON target.id = quoted.account_id \
                WHERE quote.id = $1 AND quote.status_id = $2 AND quote.quoted_status_id = $3 \
                  AND quote.activity_uri = $4 AND quote.state = 0 \
                  AND quote.approval_uri IS NULL AND quoting.deleted_at IS NULL \
                  AND quoted.deleted_at IS NULL AND quoting.local IS TRUE \
                  AND quoting.account_id = $5 AND source.domain IS NULL \
                  AND source.suspended_at IS NULL \
                  AND target.domain IS NOT NULL AND target.protocol = 1 \
                  AND target.suspended_at IS NULL \
                  AND quote.quoted_account_id = target.id AND $6 = $9 \
                FOR SHARE OF quote",
            )
            .bind(quote_id)
            .bind(quoting_status_id)
            .bind(identity.quoted_status_id)
            .bind(&identity.request_uri)
            .bind(source_account_id)
            .bind(request_actor_uri)
            .bind(request_instrument_uri)
            .bind(request_target_uri)
            .bind(source_actor_uri)
            .bind(origin)
            .fetch_optional(&mut **transaction)
            .await?;
            Ok(row.is_some_and(
                |(instrument_matches, target_matches, _, direct_inbox, shared_inbox, domain)| {
                    instrument_matches
                        && target_matches
                        && (inbox_url == direct_inbox || inbox_url == shared_inbox)
                        && configured_remote_domain.is_none_or(|value| value == domain)
                },
            ))
        }
        QuoteDeliveryKind::Accept | QuoteDeliveryKind::Reject
            if identity.quote_id.is_some() && identity.quoting_status_id.is_some() =>
        {
            let expected_state = if identity.kind == QuoteDeliveryKind::Accept {
                1
            } else {
                2
            };
            let row = sqlx::query_as::<_, (bool, bool, String, String, String, String)>(
                "SELECT ($7 = instrument.uri OR $7 = instrument.url), \
                        ($8 = target.uri OR $8 = target.url OR \
                         $8 = $10 || '/actor/statuses/' || target.id::text OR \
                         $8 = $10 || '/@' || source.username || '/' || target.id::text OR \
                         $8 = $10 || '/users/' || source.username || '/statuses/' || target.id::text OR \
                         $8 = $10 || '/ap/users/' || source.id::text || '/statuses/' || target.id::text), \
                        requester.uri, requester.inbox_url, requester.shared_inbox_url, requester.domain \
                   FROM quotes quote \
                   JOIN statuses instrument ON instrument.id = quote.status_id \
                   JOIN accounts requester ON requester.id = instrument.account_id \
                   JOIN statuses target ON target.id = quote.quoted_status_id \
                   JOIN accounts source ON source.id = target.account_id \
                  WHERE quote.id = $1 AND quote.status_id = $2 AND quote.quoted_status_id = $3 \
                    AND quote.activity_uri = $4 \
                    AND (($11 = 1 AND quote.state = 1) \
                      OR ($11 = 2 AND quote.state IN (2, 3))) \
                    AND quote.approval_uri IS NULL AND quote.quoted_account_id = $5 \
                    AND instrument.deleted_at IS NULL AND instrument.local IS NOT TRUE \
                    AND target.deleted_at IS NULL AND target.local IS TRUE \
                    AND target.account_id = $5 AND source.domain IS NULL \
                    AND source.suspended_at IS NULL \
                    AND requester.domain IS NOT NULL AND requester.protocol = 1 \
                    AND requester.suspended_at IS NULL AND $6 = $9 \
                  FOR SHARE OF quote",
            )
            .bind(identity.quote_id.expect("checked above"))
            .bind(identity.quoting_status_id.expect("checked above"))
            .bind(identity.quoted_status_id)
            .bind(&identity.request_uri)
            .bind(source_account_id)
            .bind(request_actor_uri)
            .bind(request_instrument_uri)
            .bind(request_target_uri)
            .bind(source_actor_uri)
            .bind(origin)
            .bind(expected_state)
            .fetch_optional(&mut **transaction)
            .await?;
            let Some((
                instrument_matches,
                target_matches,
                requester_uri,
                direct_inbox,
                shared_inbox,
                domain,
            )) = row
            else {
                return Ok(false);
            };
            if !instrument_matches
                || !target_matches
                || request_actor_uri != requester_uri
                || (inbox_url != direct_inbox && inbox_url != shared_inbox)
                || configured_remote_domain.is_some_and(|value| value != domain)
            {
                return Ok(false);
            }
            if identity.kind == QuoteDeliveryKind::Accept {
                let expected_authorization = format!(
                    "{}/quote_authorizations/{quote_id}",
                    source_actor_uri.trim_end_matches('/'),
                    quote_id = identity.quote_id.expect("checked above")
                );
                if quote_body_uri(body.get("result")) != Some(expected_authorization.as_str()) {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        QuoteDeliveryKind::Reject => {
            let row = sqlx::query_as::<_, (bool, String, String, String, String)>(
                "SELECT ($3 = target.uri OR $3 = target.url OR \
                         $3 = $5 || '/actor/statuses/' || target.id::text OR \
                         $3 = $5 || '/@' || source.username || '/' || target.id::text OR \
                         $3 = $5 || '/users/' || source.username || '/statuses/' || target.id::text OR \
                         $3 = $5 || '/ap/users/' || source.id::text || '/statuses/' || target.id::text), \
                        requester.uri, requester.inbox_url, requester.shared_inbox_url, requester.domain \
                   FROM statuses target \
                   JOIN accounts source ON source.id = target.account_id \
                   JOIN accounts requester ON requester.uri = $4 AND requester.domain IS NOT NULL \
                    AND requester.protocol = 1 AND requester.suspended_at IS NULL \
                  WHERE target.id = $1 AND target.account_id = $2 \
                    AND target.deleted_at IS NULL AND target.local IS TRUE \
                    AND source.domain IS NULL AND source.suspended_at IS NULL \
                  ORDER BY requester.id LIMIT 1",
            )
            .bind(identity.quoted_status_id)
            .bind(source_account_id)
            .bind(request_target_uri)
            .bind(request_actor_uri)
            .bind(origin)
            .fetch_optional(&mut **transaction)
            .await?;
            Ok(row.is_some_and(
                |(target_matches, requester_uri, direct_inbox, shared_inbox, domain)| {
                    target_matches
                        && request_actor_uri == requester_uri
                        && (inbox_url == direct_inbox || inbox_url == shared_inbox)
                        && configured_remote_domain.is_none_or(|value| value == domain)
                        && Url::parse(request_instrument_uri).is_ok_and(|instrument| {
                            Url::parse(request_actor_uri)
                                .is_ok_and(|actor| same_url_origin(&instrument, &actor))
                        })
                },
            ))
        }
        QuoteDeliveryKind::Accept => Ok(false),
    }
}

#[allow(clippy::too_many_lines)]
pub(super) async fn deliver_activity(
    pool: PgPool,
    operational_pool: PgPool,
    config: &ActivityPubDeliveryConfig,
    fetcher: &RemoteFetcher,
    job: &ClaimedJob,
) -> Result<(), HandlerFailure> {
    let arguments = &job.arguments;
    let status_id = arguments.get("status_id").and_then(Value::as_i64);
    let source_account_id = arguments
        .get("source_account_id")
        .and_then(Value::as_i64)
        .ok_or_else(|| HandlerFailure::permanent("delivery job is missing its source account"))?;
    let inbox_url = arguments
        .get("inbox_url")
        .and_then(Value::as_str)
        .ok_or_else(|| HandlerFailure::permanent("delivery job is missing its inbox URL"))?;
    let body_value = arguments
        .get("body")
        .ok_or_else(|| HandlerFailure::permanent("delivery job is missing its activity"))?;
    if !body_value.is_object() {
        return Err(HandlerFailure::permanent(
            "delivery job activity must be a JSON object",
        ));
    }
    let Ok(quote_delivery_identity) = quote_delivery_identity(arguments, body_value) else {
        return Err(HandlerFailure::permanent(
            "quote delivery metadata does not match its activity",
        ));
    };
    let body = serde_json::to_vec(body_value)
        .map_err(|_| HandlerFailure::permanent("delivery activity could not be serialized"))?;
    let repository = Repository::from_pool(pool.clone());
    let delivery_edited_at_micros = arguments.get("edited_at_micros").and_then(Value::as_i64);
    let delivery_poll_updated_at_micros = arguments
        .get("poll_updated_at_micros")
        .and_then(Value::as_i64);
    let delivery_quote_updated_at_micros = arguments
        .get("quote_updated_at_micros")
        .and_then(Value::as_i64);
    let delivery_update_version_micros = arguments
        .get("update_version_micros")
        .and_then(Value::as_i64);
    let delivery_update_kind = arguments.get("update_kind").and_then(Value::as_str);
    let delivery_published_version_micros = body_value
        .get("published")
        .and_then(Value::as_str)
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.timestamp_micros());
    let delivery_updated_at_micros = arguments.get("updated_at_micros").and_then(Value::as_i64);
    let configured_remote_domain = arguments
        .get("remote_domain")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let is_delete = body_value.get("type").and_then(Value::as_str) == Some("Delete");
    let is_undo_announce = body_value.get("type").and_then(Value::as_str) == Some("Undo")
        && body_value
            .get("object")
            .and_then(|object| object.get("type"))
            .and_then(Value::as_str)
            == Some("Announce");
    let current_status = match status_id {
        Some(status_id) if !is_delete && !is_undo_announce => {
            let status = repository
                .status(status_id)
                .await
                .map_err(|_| HandlerFailure::retry("delivery status lookup failed"))?;
            if status.is_none() {
                return Ok(());
            }
            status
        }
        _ => None,
    };
    let source_account = repository
        .account(source_account_id)
        .await
        .map_err(|_| HandlerFailure::retry("delivery source lookup failed"))?
        .ok_or_else(|| HandlerFailure::permanent("delivery source account is missing"))?;
    if source_account.domain.is_some() {
        return Err(HandlerFailure::permanent(
            "delivery source account is not local",
        ));
    }
    let source_account_permanently_unavailable = if source_account.suspended_at.is_some() {
        !repository
            .account_has_deletion_request(source_account_id)
            .await
            .map_err(|_| HandlerFailure::retry("delivery source lifecycle lookup failed"))?
    } else {
        false
    };
    if is_delete && status_id.is_none() && source_account.suspended_at.is_none() {
        return Ok(());
    }
    if body_value.get("type").and_then(Value::as_str) == Some("Update") && status_id.is_none() {
        let actor_uri = activitypub::actor_url(&config.origin, &source_account);
        if !update_delivery_is_current(
            body_value.get("id").and_then(Value::as_str),
            &actor_uri,
            source_account.updated_at,
            delivery_updated_at_micros,
        ) {
            return Ok(());
        }
    }
    if body_value.get("type").and_then(Value::as_str) == Some("Update")
        && let Some(status) = current_status.as_ref()
    {
        let object_uri = activitypub::status_uri(&config.origin, &source_account, status);
        if body_value
            .get("object")
            .and_then(|object| object.get("id"))
            .and_then(Value::as_str)
            != Some(object_uri.as_str())
        {
            return Ok(());
        }
        let current_poll = if let Some(poll_id) = status.poll_id {
            let Some(current) = sqlx::query_scalar::<_, NaiveDateTime>(
                "SELECT updated_at FROM polls WHERE id = $1",
            )
            .bind(poll_id)
            .fetch_optional(&pool)
            .await
            .map_err(|_| HandlerFailure::retry("poll delivery fence lookup failed"))?
            else {
                return Ok(());
            };
            Some((poll_id, current))
        } else {
            None
        };
        let current_edited_at = if delivery_update_kind == Some("interaction_policy") {
            status.updated_at
        } else {
            status.edited_at.unwrap_or(status.updated_at)
        };
        let current_poll_updated_at = current_poll.map(|(_, updated_at)| updated_at);
        let quote_delivery_is_current = if delivery_update_kind == Some("quote") {
            let current_quote_updated_at = sqlx::query_scalar::<_, NaiveDateTime>(
                "SELECT updated_at FROM quotes WHERE status_id = $1 ORDER BY id LIMIT 1",
            )
            .bind(status.id)
            .fetch_optional(&pool)
            .await
            .map_err(|_| HandlerFailure::retry("quote delivery fence lookup failed"))?;
            quote_revision_is_current(current_quote_updated_at, delivery_quote_updated_at_micros)
        } else {
            true
        };
        let complete_delivery_kind = complete_status_update_delivery_kind(
            delivery_update_kind,
            current_poll.is_some(),
            delivery_edited_at_micros,
            delivery_poll_updated_at_micros,
            delivery_update_version_micros,
            delivery_published_version_micros,
        );
        let inferred_repair_kind = complete_delivery_kind.is_none().then(|| {
            inferred_current_repair_delivery_kind(
                body_value["id"].as_str(),
                &object_uri,
                current_edited_at,
                current_poll_updated_at,
                delivery_published_version_micros,
            )
        });
        let delivery_is_current = quote_delivery_is_current
            && (complete_delivery_kind.is_some_and(|delivery_kind| {
                status_update_delivery_is_current(
                    delivery_kind,
                    body_value["id"].as_str(),
                    &object_uri,
                    current_edited_at,
                    current_poll_updated_at,
                    delivery_edited_at_micros,
                    delivery_poll_updated_at_micros,
                    delivery_update_version_micros,
                )
            }) || inferred_repair_kind.flatten().is_some());
        if !delivery_is_current {
            if delivery_update_kind == Some("quote") {
                return Ok(());
            }
            queue_status_snapshot_repair(
                &pool,
                status.id,
                current_poll.map(|(poll_id, _)| poll_id),
                current_edited_at,
                current_poll_updated_at,
            )
            .await?;
            return Ok(());
        }
    }
    let private_key = source_account
        .private_key
        .as_ref()
        .filter(|key| key.is_present())
        .ok_or_else(|| HandlerFailure::permanent("delivery source has no private key"))?;
    let source_actor_uri = activitypub::actor_url(&config.origin, &source_account);
    let key_id = format!("{source_actor_uri}#main-key");
    let signer = HttpSignatureSigner {
        key_id: &key_id,
        private_key_pem: private_key.as_str(),
    };
    let inbox_url = Url::parse(inbox_url)
        .map_err(|_| HandlerFailure::permanent("delivery inbox URL is invalid"))?;
    let domain = inbox_url
        .host_str()
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| HandlerFailure::permanent("delivery inbox URL has no host"))?;
    let policy_domain = configured_remote_domain.as_deref().unwrap_or(&domain);
    if !repository
        .remote_domain_allowed(policy_domain, config.limited_federation)
        .await
        .map_err(|_| HandlerFailure::retry("delivery remote-domain policy lookup failed"))?
    {
        return Ok(());
    }
    if !relationship_delivery_is_current(&pool, body_value).await? {
        return Ok(());
    }
    if !delivery_domain_available(&operational_pool, &domain).await? {
        return Err(HandlerFailure::retry(
            "remote delivery domain is cooling down",
        ));
    }
    let delivery = if let Some(identity) = quote_delivery_identity.as_ref() {
        let mut transaction = pool
            .begin()
            .await
            .map_err(|_| HandlerFailure::retry("quote delivery fence transaction failed"))?;
        let current = quote_delivery_is_current_and_locked(
            &mut transaction,
            identity,
            body_value,
            source_account_id,
            &source_actor_uri,
            inbox_url.as_str(),
            configured_remote_domain.as_deref(),
            config.origin.as_str(),
        )
        .await
        .map_err(|_| HandlerFailure::retry("quote delivery fence lookup failed"))?;
        if !current {
            transaction
                .rollback()
                .await
                .map_err(|_| HandlerFailure::retry("quote delivery fence rollback failed"))?;
            return Ok(());
        }
        if !WriteRepository::remote_domain_allowed_in_transaction(
            &mut transaction,
            policy_domain,
            config.limited_federation,
        )
        .await
        .map_err(|_| HandlerFailure::retry("quote delivery policy fence failed"))?
        {
            transaction
                .rollback()
                .await
                .map_err(|_| HandlerFailure::retry("quote delivery fence rollback failed"))?;
            return Ok(());
        }
        let lease_fenced = sqlx::query(
            "UPDATE rustodon.durable_jobs \
                SET lease_expires_at = GREATEST(lease_expires_at, \
                    clock_timestamp() + interval '60 seconds'), \
                    updated_at = clock_timestamp() \
              WHERE id = $1 AND lease_owner = $2 AND lease_generation = $3 \
                AND dead_at IS NULL AND lease_expires_at > clock_timestamp()",
        )
        .bind(job.id)
        .bind(&job.lease_owner)
        .bind(job.generation)
        .execute(&mut *transaction)
        .await
        .map_err(|_| HandlerFailure::retry("quote delivery lease fence failed"))?
        .rows_affected()
            == 1;
        if !lease_fenced {
            transaction
                .rollback()
                .await
                .map_err(|_| HandlerFailure::retry("quote delivery fence rollback failed"))?;
            return Ok(());
        }
        #[cfg(feature = "test-support")]
        let delivery = if let Some(endpoint) = config.remote_delivery_endpoint {
            fetcher
                .post_signed_json_for_test_endpoint(inbox_url.clone(), &body, &signer, endpoint)
                .await
        } else {
            fetcher
                .post_signed_json(inbox_url.clone(), &body, &signer)
                .await
        };
        #[cfg(not(feature = "test-support"))]
        let delivery = fetcher
            .post_signed_json(inbox_url.clone(), &body, &signer)
            .await;
        if delivery.is_ok() {
            transaction
                .commit()
                .await
                .map_err(|_| HandlerFailure::retry("quote delivery fence commit failed"))?;
        } else {
            transaction
                .rollback()
                .await
                .map_err(|_| HandlerFailure::retry("quote delivery fence rollback failed"))?;
        }
        delivery
    } else if is_delete && status_id.is_none() {
        let writer = WriteRepository::from_pool(pool.clone());
        let delivery = writer
            .with_account_lock(source_account_id, || async {
                let mut transaction = writer.pool().begin().await?;
                let current =
                    account_delete_delivery_is_current(&mut transaction, source_account_id).await?;
                transaction.commit().await?;
                if !current {
                    return Ok(None);
                }
                #[cfg(feature = "test-support")]
                let delivery = if let Some(endpoint) = config.remote_delivery_endpoint {
                    fetcher
                        .post_signed_json_for_test_endpoint(
                            inbox_url.clone(),
                            &body,
                            &signer,
                            endpoint,
                        )
                        .await
                } else {
                    fetcher
                        .post_signed_json(inbox_url.clone(), &body, &signer)
                        .await
                };
                #[cfg(not(feature = "test-support"))]
                let delivery = fetcher
                    .post_signed_json(inbox_url.clone(), &body, &signer)
                    .await;
                Ok(Some(delivery))
            })
            .await
            .map_err(|_| HandlerFailure::retry("account deletion delivery lock failed"))?;
        let Some(delivery) = delivery else {
            return Ok(());
        };
        delivery
    } else {
        #[cfg(feature = "test-support")]
        if let Some(endpoint) = config.remote_delivery_endpoint {
            fetcher
                .post_signed_json_for_test_endpoint(inbox_url.clone(), &body, &signer, endpoint)
                .await
        } else {
            fetcher
                .post_signed_json(inbox_url.clone(), &body, &signer)
                .await
        }
        #[cfg(not(feature = "test-support"))]
        {
            fetcher
                .post_signed_json(inbox_url.clone(), &body, &signer)
                .await
        }
    };
    match delivery {
        Ok(_) => {
            record_domain_success(&operational_pool, &domain).await?;
            Ok(())
        }
        Err(error) => {
            let failure = delivery_failure(&error, source_account_permanently_unavailable);
            if failure.disposition == FailureDisposition::Retry
                && !matches!(error, RemoteFetchError::DomainBudgetExceeded)
            {
                record_domain_failure(&operational_pool, &domain).await?;
            }
            Err(failure)
        }
    }
}

pub(super) async fn delivery_domain_available(
    pool: &PgPool,
    domain: &str,
) -> Result<bool, HandlerFailure> {
    sqlx::query_scalar(
        "SELECT COALESCE( \
           (SELECT retry_at <= clock_timestamp() FROM rustodon.domain_health WHERE domain = $1), \
           true)",
    )
    .bind(domain)
    .fetch_one(pool)
    .await
    .map_err(|_| HandlerFailure::retry("delivery domain health lookup failed"))
}

pub(super) async fn relationship_delivery_is_current(
    pool: &PgPool,
    body: &Value,
) -> Result<bool, HandlerFailure> {
    let Some(activity_type) = body.get("type").and_then(Value::as_str) else {
        return Ok(true);
    };
    let Some(activity_uri) = body.get("id").and_then(Value::as_str) else {
        return Ok(true);
    };
    let query = match activity_type {
        "Follow" => {
            "SELECT EXISTS (
               SELECT 1 FROM follows WHERE uri = $1
               UNION ALL
               SELECT 1 FROM follow_requests WHERE uri = $1)"
        }
        "Block" => "SELECT EXISTS (SELECT 1 FROM blocks WHERE uri = $1)",
        _ => return Ok(true),
    };
    sqlx::query_scalar(query)
        .bind(activity_uri)
        .fetch_one(pool)
        .await
        .map_err(|_| HandlerFailure::retry("relationship delivery state lookup failed"))
}

pub(super) async fn record_domain_success(
    pool: &PgPool,
    domain: &str,
) -> Result<(), HandlerFailure> {
    sqlx::query(
        "INSERT INTO rustodon.domain_health \
           (domain, failures, last_success_at, retry_at, last_error) \
         VALUES ($1, 0, clock_timestamp(), NULL, NULL) \
         ON CONFLICT (domain) DO UPDATE SET failures = 0, \
           last_success_at = clock_timestamp(), retry_at = NULL, last_error = NULL, \
           updated_at = clock_timestamp()",
    )
    .bind(domain)
    .execute(pool)
    .await
    .map(|_| ())
    .map_err(|_| HandlerFailure::retry("delivery domain health update failed"))
}

pub(super) async fn record_domain_failure(
    pool: &PgPool,
    domain: &str,
) -> Result<(), HandlerFailure> {
    sqlx::query(
        "INSERT INTO rustodon.domain_health \
           (domain, failures, last_failure_at, retry_at, last_error) \
         VALUES ($1, 1, clock_timestamp(), clock_timestamp() + interval '15 seconds', \
                 'remote delivery failed') \
         ON CONFLICT (domain) DO UPDATE SET \
           failures = rustodon.domain_health.failures + 1, \
           last_failure_at = clock_timestamp(), \
           retry_at = clock_timestamp() + make_interval( \
             secs => LEAST(3600, 15 * (rustodon.domain_health.failures + 1))), \
           last_error = 'remote delivery failed', updated_at = clock_timestamp()",
    )
    .bind(domain)
    .execute(pool)
    .await
    .map(|_| ())
    .map_err(|_| HandlerFailure::retry("delivery domain health update failed"))
}

pub(super) fn delivery_failure(
    error: &RemoteFetchError,
    source_account_permanently_unavailable: bool,
) -> HandlerFailure {
    match error {
        RemoteFetchError::UnexpectedStatus(status)
            if source_account_permanently_unavailable && *status == StatusCode::UNAUTHORIZED =>
        {
            HandlerFailure::permanent("remote delivery authorization is permanently unavailable")
        }
        RemoteFetchError::UnexpectedStatus(status)
            if *status == StatusCode::NOT_IMPLEMENTED
                || (status.is_client_error()
                    && !matches!(
                        *status,
                        StatusCode::UNAUTHORIZED
                            | StatusCode::REQUEST_TIMEOUT
                            | StatusCode::TOO_MANY_REQUESTS
                    )) =>
        {
            HandlerFailure::permanent("remote inbox rejected the activity")
        }
        RemoteFetchError::InvalidUrl
        | RemoteFetchError::Redirect
        | RemoteFetchError::TooManyRedirects
        | RemoteFetchError::UnsupportedEncoding
        | RemoteFetchError::BodyTooLarge
        | RemoteFetchError::IdentityMismatch
        | RemoteFetchError::OriginMismatch
        | RemoteFetchError::PolicyDenied
        | RemoteFetchError::Signing => HandlerFailure::permanent("remote delivery is invalid"),
        _ => HandlerFailure::retry("remote delivery failed"),
    }
}

pub(super) fn inbox_remote_failure(error: &RemoteFetchError) -> HandlerFailure {
    match error {
        RemoteFetchError::InvalidUrl
        | RemoteFetchError::Redirect
        | RemoteFetchError::TooManyRedirects
        | RemoteFetchError::MissingContentType
        | RemoteFetchError::UnsupportedContentType
        | RemoteFetchError::UnsupportedEncoding
        | RemoteFetchError::BodyTooLarge
        | RemoteFetchError::InvalidRepresentation
        | RemoteFetchError::IdentityMismatch
        | RemoteFetchError::OriginMismatch
        | RemoteFetchError::PolicyDenied
        | RemoteFetchError::Signing
        | RemoteFetchError::BlockedAddress(_) => {
            HandlerFailure::permanent(format!("remote inbox actor is invalid: {error}"))
        }
        RemoteFetchError::UnexpectedStatus(status)
            if status.is_client_error()
                && !matches!(
                    *status,
                    StatusCode::UNAUTHORIZED
                        | StatusCode::REQUEST_TIMEOUT
                        | StatusCode::TOO_MANY_REQUESTS
                ) =>
        {
            HandlerFailure::permanent(format!("remote inbox actor could not be resolved: {error}"))
        }
        _ => HandlerFailure::retry(format!("remote inbox actor resolution failed: {error}")),
    }
}
