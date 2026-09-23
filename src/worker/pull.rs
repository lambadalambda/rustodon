//! Remote fetches: reply threads, notes, announces, media and emoji.

use super::*;

pub(super) fn remote_thread_fetch_failure(error: &RemoteFetchError) -> HandlerFailure {
    match error {
        RemoteFetchError::UnexpectedStatus(status)
            if *status == StatusCode::NOT_FOUND
                || *status == StatusCode::REQUEST_TIMEOUT
                || *status == StatusCode::TOO_MANY_REQUESTS
                || status.is_server_error() =>
        {
            HandlerFailure::retry(format!(
                "remote reply parent fetch is temporarily unavailable: {error}"
            ))
        }
        RemoteFetchError::UnexpectedStatus(_) => {
            HandlerFailure::permanent(format!("remote reply parent fetch was rejected: {error}"))
        }
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
            HandlerFailure::permanent(format!("remote reply parent is invalid: {error}"))
        }
        RemoteFetchError::NoAddresses
        | RemoteFetchError::Dns
        | RemoteFetchError::Client
        | RemoteFetchError::Request
        | RemoteFetchError::BodyRead
        | RemoteFetchError::DomainBudgetExceeded => {
            HandlerFailure::retry(format!("remote reply parent fetch failed: {error}"))
        }
    }
}

pub(super) fn remote_announce_fetch_failure(error: &RemoteFetchError) -> HandlerFailure {
    match error {
        RemoteFetchError::UnexpectedStatus(status)
            if *status == StatusCode::NOT_FOUND
                || *status == StatusCode::REQUEST_TIMEOUT
                || *status == StatusCode::TOO_MANY_REQUESTS
                || status.is_server_error() =>
        {
            HandlerFailure::retry(format!(
                "remote Announce target fetch is temporarily unavailable: {error}"
            ))
        }
        RemoteFetchError::UnexpectedStatus(_) => HandlerFailure::permanent(format!(
            "remote Announce target fetch was rejected: {error}"
        )),
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
            HandlerFailure::permanent(format!("remote Announce target is invalid: {error}"))
        }
        RemoteFetchError::NoAddresses
        | RemoteFetchError::Dns
        | RemoteFetchError::Client
        | RemoteFetchError::Request
        | RemoteFetchError::BodyRead
        | RemoteFetchError::DomainBudgetExceeded => {
            HandlerFailure::retry(format!("remote Announce target fetch failed: {error}"))
        }
    }
}

pub(super) fn remote_thread_write_failure(error: &WriteError) -> HandlerFailure {
    match error {
        WriteError::Sqlx(_)
        | WriteError::Job(_)
        | WriteError::Filesystem(_)
        | WriteError::NotFound => HandlerFailure::retry("remote reply thread persistence failed"),
        WriteError::Conflict
        | WriteError::InvalidInput(_)
        | WriteError::Unauthorized
        | WriteError::Forbidden
        | WriteError::RateLimited
        | WriteError::Validation(_) => {
            HandlerFailure::permanent("remote reply thread payload is invalid")
        }
    }
}

pub(super) fn remote_uri_value(value: Option<&Value>) -> Option<&str> {
    match value {
        Some(Value::String(value)) => Some(value.as_str()),
        Some(Value::Object(object)) => object.get("id").and_then(Value::as_str),
        Some(Value::Array(values)) => remote_uri_value(values.first()),
        _ => None,
    }
}

pub(super) fn announce_resolution_logical_key(activity_uri: &str) -> String {
    let digest = Sha256::digest(activity_uri.as_bytes());
    let mut digest_string = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut digest_string, "{byte:02x}").expect("writing to a String cannot fail");
    }
    format!("activitypub:announce:{digest_string}")
}

pub(super) fn note_resolution_logical_key(
    source_account_id: i64,
    actor_uri: &str,
    object_uri: &str,
    delivery_target_account_id: Option<i64>,
) -> String {
    let digest = Sha256::digest(
        format!(
            "{source_account_id}\n{actor_uri}\n{object_uri}\n{}",
            delivery_target_account_id.map_or_else(|| "shared".to_owned(), |id| id.to_string())
        )
        .as_bytes(),
    );
    let mut digest_string = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut digest_string, "{byte:02x}").expect("writing to a String cannot fail");
    }
    format!("activitypub:note:{digest_string}")
}

pub(super) fn validate_create_binding(
    activity_uri: &str,
    actor_uri: &str,
    object_uri: &str,
) -> Result<(), HandlerFailure> {
    Url::parse(activity_uri)
        .map_err(|_| HandlerFailure::permanent("remote Create activity URI is invalid"))?;
    let actor_location = Url::parse(actor_uri)
        .map_err(|_| HandlerFailure::permanent("remote Create actor URI is invalid"))?;
    let object_location = Url::parse(object_uri)
        .map_err(|_| HandlerFailure::permanent("remote Create object URI is invalid"))?;
    let actor_host = actor_location.host_str();
    if actor_host.is_none()
        || object_location
            .host_str()
            .zip(actor_host)
            .is_none_or(|(object_host, actor_host)| !object_host.eq_ignore_ascii_case(actor_host))
    {
        return Err(HandlerFailure::permanent(
            "remote Create actor and object hosts do not match",
        ));
    }
    Ok(())
}

pub(super) fn resolved_create_note(
    document: &Value,
    activity_uri: &str,
    actor_uri: &str,
    object_uri: &str,
) -> Result<Value, HandlerFailure> {
    validate_create_binding(activity_uri, actor_uri, object_uri)?;
    if !["Note", "Question"]
        .into_iter()
        .any(|kind| equals_or_includes(document.get("type"), kind))
    {
        return Err(HandlerFailure::permanent(
            "remote Create object is not a Note or Question",
        ));
    }
    if remote_uri_value(document.get("id")) != Some(object_uri) {
        return Err(HandlerFailure::permanent(
            "remote Create object ID does not match the requested URI",
        ));
    }
    let object = document
        .as_object()
        .ok_or_else(|| HandlerFailure::permanent("remote Create Note is not an object"))?;
    validate_note_object(actor_uri, object)
        .map_err(|_| HandlerFailure::permanent("remote Create Note is invalid"))?;
    Ok(document.clone())
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn schedule_remote_note_resolution(
    pool: &PgPool,
    source_account_id: i64,
    activity_uri: &str,
    actor_uri: &str,
    object_uri: &str,
    to: &[String],
    cc: &[String],
    delivery_target_account_id: Option<i64>,
    activity: &Value,
) -> Result<(), HandlerFailure> {
    let job = JobSpec::new(
        Lane::Pull,
        ACTIVITYPUB_NOTE_RESOLVE_JOB_KIND,
        json!({
            "source_account_id": source_account_id,
            "activity_uri": activity_uri,
            "actor_uri": actor_uri,
            "object_uri": object_uri,
            "to": to,
            "cc": cc,
            "delivery_target_account_id": delivery_target_account_id,
            "activity": activity
        }),
    )
    .logical_key(note_resolution_logical_key(
        source_account_id,
        actor_uri,
        object_uri,
        delivery_target_account_id,
    ));
    let mut transaction = pool
        .begin()
        .await
        .map_err(|_| HandlerFailure::retry("remote Note resolution outbox transaction failed"))?;
    record_outbox_once_in(&mut transaction, &job)
        .await
        .map_err(|_| HandlerFailure::retry("remote Note resolution outbox write failed"))?;
    transaction
        .commit()
        .await
        .map_err(|_| HandlerFailure::retry("remote Note resolution outbox commit failed"))?;
    Ok(())
}

pub(super) fn remote_note_fetch_failure(error: &RemoteFetchError) -> HandlerFailure {
    match remote_announce_fetch_failure(error).disposition {
        FailureDisposition::Retry => {
            HandlerFailure::retry(format!("remote Create Note fetch failed: {error}"))
        }
        FailureDisposition::Permanent => {
            HandlerFailure::permanent(format!("remote Create Note fetch is invalid: {error}"))
        }
    }
}

pub(super) fn remote_quote_request_fetch_failure(error: &RemoteFetchError) -> HandlerFailure {
    match error {
        RemoteFetchError::UnexpectedStatus(status)
            if *status == StatusCode::NOT_FOUND
                || *status == StatusCode::REQUEST_TIMEOUT
                || *status == StatusCode::TOO_MANY_REQUESTS
                || status.is_server_error() =>
        {
            HandlerFailure::retry(format!(
                "QuoteRequest instrument is temporarily unavailable: {error}"
            ))
        }
        RemoteFetchError::UnexpectedStatus(_) => HandlerFailure::permanent(format!(
            "QuoteRequest instrument fetch was rejected: {error}"
        )),
        RemoteFetchError::NoAddresses
        | RemoteFetchError::Dns
        | RemoteFetchError::Client
        | RemoteFetchError::Request
        | RemoteFetchError::BodyRead
        | RemoteFetchError::DomainBudgetExceeded => {
            HandlerFailure::retry(format!("QuoteRequest instrument fetch failed: {error}"))
        }
        RemoteFetchError::InvalidUrl
        | RemoteFetchError::BlockedAddress(_)
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
        | RemoteFetchError::Signing => {
            HandlerFailure::permanent(format!("QuoteRequest instrument fetch is invalid: {error}"))
        }
    }
}

pub(super) async fn fetch_quote_request_instrument(
    repository: &Repository,
    config: &ActivityPubDeliveryConfig,
    fetcher: &RemoteFetcher,
    target_account_id: i64,
    instrument_uri: &str,
) -> Result<Value, HandlerFailure> {
    let parsed_instrument = Url::parse(instrument_uri)
        .map_err(|_| HandlerFailure::permanent("QuoteRequest instrument URL is invalid"))?;
    let signer_account = repository
        .account(target_account_id)
        .await
        .map_err(|_| HandlerFailure::retry("QuoteRequest fetch signer lookup failed"))?
        .filter(|account| account.domain.is_none())
        .ok_or_else(|| HandlerFailure::permanent("QuoteRequest fetch signer is unavailable"))?;
    let private_key = signer_account
        .private_key
        .as_ref()
        .filter(|key| key.is_present())
        .ok_or_else(|| HandlerFailure::permanent("QuoteRequest fetch signer has no private key"))?;
    let signer_key_id = format!(
        "{}#main-key",
        activitypub::actor_url(&config.origin, &signer_account)
    );
    let signer = HttpSignatureSigner {
        key_id: &signer_key_id,
        private_key_pem: private_key.as_str(),
    };
    #[cfg(feature = "test-support")]
    let fetcher = fetcher
        .clone()
        .with_test_endpoint(config.remote_fetch_endpoint);
    #[cfg(not(feature = "test-support"))]
    let fetcher = fetcher.clone();
    let response = fetcher
        .get_signed(parsed_instrument, THREAD_ACTIVITYPUB_CONTENT_TYPES, &signer)
        .await
        .map_err(|error| remote_quote_request_fetch_failure(&error))?;
    serde_json::from_slice(&response.body)
        .map_err(|_| HandlerFailure::permanent("QuoteRequest instrument JSON is invalid"))
}

#[allow(clippy::too_many_lines)]
pub(super) async fn process_activitypub_note_resolution(
    pool: PgPool,
    config: &ActivityPubDeliveryConfig,
    fetcher: &RemoteFetcher,
    arguments: &Value,
) -> Result<(), HandlerFailure> {
    let source_account_id = arguments
        .get("source_account_id")
        .and_then(Value::as_i64)
        .ok_or_else(|| HandlerFailure::permanent("Note resolution job is missing its source"))?;
    let activity_uri = arguments
        .get("activity_uri")
        .and_then(Value::as_str)
        .ok_or_else(|| HandlerFailure::permanent("Note resolution job is missing its activity"))?;
    let actor_uri = arguments
        .get("actor_uri")
        .and_then(Value::as_str)
        .ok_or_else(|| HandlerFailure::permanent("Note resolution job is missing its actor"))?;
    let object_uri = arguments
        .get("object_uri")
        .and_then(Value::as_str)
        .ok_or_else(|| HandlerFailure::permanent("Note resolution job is missing its object"))?;
    let activity = arguments
        .get("activity")
        .ok_or_else(|| HandlerFailure::permanent("Note resolution job is missing its payload"))?;
    let parsed_activity = parse_activity(&activity.to_string())
        .map_err(|_| HandlerFailure::permanent("Note resolution activity is invalid"))?;
    let InboxActivity::CreateNoteReference {
        activity_uri: parsed_activity_uri,
        actor_uri: parsed_actor_uri,
        object_uri: parsed_object_uri,
        to,
        cc,
        ..
    } = parsed_activity
    else {
        return Err(HandlerFailure::permanent(
            "Note resolution payload is not a URI-only Create",
        ));
    };
    if parsed_activity_uri != activity_uri
        || parsed_actor_uri != actor_uri
        || parsed_object_uri != object_uri
        || arguments.get("to") != Some(&json!(to))
        || arguments.get("cc") != Some(&json!(cc))
    {
        return Err(HandlerFailure::permanent(
            "Note resolution contract does not match its activity",
        ));
    }
    validate_create_binding(activity_uri, actor_uri, object_uri)?;
    let delivery_target_account_id = parse_delivery_target_account_id(arguments)?;
    let repository = Repository::from_pool(pool.clone());
    let writer = WriteRepository::from_pool(pool.clone());
    let resolved = writer
        .remote_note_reference_is_resolved(source_account_id, actor_uri, object_uri)
        .await
        .map_err(|error| {
            remote_note_write_failure(&error, "remote Note resolution state lookup failed")
        })?;
    if resolved && let Some(delivery_target_account_id) = delivery_target_account_id {
        writer
            .ensure_remote_note_reference_delivery(
                source_account_id,
                actor_uri,
                object_uri,
                delivery_target_account_id,
            )
            .await
            .map_err(|error| {
                remote_note_write_failure(&error, "remote Note delivery target repair failed")
            })?;
    }
    let target = Url::parse(object_uri)
        .map_err(|_| HandlerFailure::permanent("remote Create object URI is invalid"))?;
    if same_url_origin(&target, &config.origin) {
        return Err(HandlerFailure::permanent(
            "remote Create object URI is local",
        ));
    }
    let object_domain = inbox_actor_domain(object_uri)
        .ok_or_else(|| HandlerFailure::permanent("remote Create object has no valid domain"))?;
    if !repository
        .remote_domain_allowed(&object_domain, config.limited_federation)
        .await
        .map_err(|_| HandlerFailure::retry("remote Create object policy lookup failed"))?
    {
        return Err(HandlerFailure::permanent(
            "remote Create object domain is not allowed",
        ));
    }
    let signer_account = resolve_note_fetch_signer(
        &pool,
        source_account_id,
        delivery_target_account_id,
        &to,
        &cc,
        &config.origin,
    )
    .await?;
    let private_key = signer_account
        .private_key
        .as_ref()
        .filter(|key| key.is_present())
        .ok_or_else(|| HandlerFailure::permanent("remote Create signer has no private key"))?;
    let signer_key_id = format!(
        "{}#main-key",
        activitypub::actor_url(&config.origin, &signer_account)
    );
    let signer = HttpSignatureSigner {
        key_id: &signer_key_id,
        private_key_pem: private_key.as_str(),
    };
    let response = {
        #[cfg(feature = "test-support")]
        if let Some(endpoint) = config.remote_fetch_endpoint {
            fetcher
                .get_for_test_endpoint(target.clone(), THREAD_ACTIVITYPUB_CONTENT_TYPES, endpoint)
                .await
        } else {
            fetcher
                .get_signed(target.clone(), THREAD_ACTIVITYPUB_CONTENT_TYPES, &signer)
                .await
        }
        #[cfg(not(feature = "test-support"))]
        fetcher
            .get_signed(target.clone(), THREAD_ACTIVITYPUB_CONTENT_TYPES, &signer)
            .await
    }
    .map_err(|error| remote_note_fetch_failure(&error))?;
    if !same_url_origin(&response.url, &target) {
        return Err(HandlerFailure::permanent(
            "remote Create object redirected to another origin",
        ));
    }
    let document = serde_json::from_slice::<Value>(&response.body)
        .map_err(|_| HandlerFailure::permanent("remote Create object JSON is invalid"))?;
    let object = resolved_create_note(&document, activity_uri, actor_uri, object_uri)?;
    if !writer
        .remote_note_is_relevant(
            source_account_id,
            actor_uri,
            &object,
            delivery_target_account_id,
            config.origin.as_str(),
        )
        .await
        .map_err(|error| remote_note_write_failure(&error, "remote Note relevance check failed"))?
    {
        return Ok(());
    }
    let quote_authorization = fetch_and_import_quote_authorization(
        &pool,
        &repository,
        &writer,
        config,
        fetcher,
        source_account_id,
        delivery_target_account_id,
        &object,
    )
    .await?;
    if resolved {
        writer
            .apply_remote_note_update(
                source_account_id,
                actor_uri,
                &object,
                delivery_target_account_id,
                config.origin.as_str(),
            )
            .await
            .map_err(|error| {
                remote_note_write_failure(&error, "resolved remote Note replay write failed")
            })?;
    } else {
        writer
            .apply_remote_note_create(
                source_account_id,
                actor_uri,
                &object,
                delivery_target_account_id,
                config.origin.as_str(),
            )
            .await
            .map_err(|error| {
                remote_note_write_failure(&error, "resolved remote Note write failed")
            })?;
    }
    if let Some((references, Some(document))) = quote_authorization {
        let approval_uri = references
            .approval_uri
            .as_deref()
            .expect("fetched authorization has a canonical URI");
        writer
            .apply_remote_quote_authorization(
                source_account_id,
                object_uri,
                approval_uri,
                &document,
                config.origin.as_str(),
            )
            .await
            .map_err(|error| {
                remote_note_write_failure(&error, "resolved remote QuoteAuthorization write failed")
            })?;
    }
    if activity
        .get("signature")
        .is_some_and(|signature| !signature.is_null())
    {
        writer
            .record_remote_note_reference_forwarding(actor_uri, object_uri, activity)
            .await
            .map_err(|error| {
                remote_note_write_failure(&error, "resolved remote Note forwarding failed")
            })?;
    }
    Ok(())
}

pub(super) fn parse_delivery_target_account_id(
    arguments: &Value,
) -> Result<Option<i64>, HandlerFailure> {
    match arguments.get("delivery_target_account_id") {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_i64()
            .map(Some)
            .ok_or_else(|| HandlerFailure::permanent("Note resolution delivery target is invalid")),
    }
}

pub(super) fn preferred_note_fetch_signer_id(
    delivery_target_account_id: Option<i64>,
    addressed_account_id: Option<i64>,
    follower_account_id: Option<i64>,
) -> Option<i64> {
    delivery_target_account_id
        .or(addressed_account_id)
        .or(follower_account_id)
}

pub(super) fn note_fetch_audience<'a>(to: &'a [String], cc: &'a [String]) -> Vec<&'a String> {
    to.iter().chain(cc).collect()
}

pub(super) async fn resolve_note_fetch_signer(
    pool: &PgPool,
    source_account_id: i64,
    delivery_target_account_id: Option<i64>,
    to: &[String],
    cc: &[String],
    origin: &Url,
) -> Result<Account, HandlerFailure> {
    let valid_delivery_target = sqlx::query_scalar::<_, i64>(
        "SELECT id FROM accounts
          WHERE id = $1 AND domain IS NULL
            AND private_key IS NOT NULL AND private_key <> ''",
    )
    .bind(delivery_target_account_id)
    .fetch_optional(pool)
    .await
    .map_err(|_| HandlerFailure::retry("remote Create delivery signer lookup failed"))?;
    let audience = note_fetch_audience(to, cc);
    let addressed_account_id = sqlx::query_scalar::<_, i64>(
        "SELECT account.id
           FROM unnest($1::text[]) WITH ORDINALITY AS audience(uri, position)
           JOIN accounts account ON account.domain IS NULL
             AND account.private_key IS NOT NULL AND account.private_key <> ''
             AND (
               account.uri = audience.uri OR account.url = audience.uri
               OR (audience.uri = $2 || '/actor' AND account.id = -99)
               OR audience.uri = $2 || '/@' || account.username
               OR (audience.uri = $2 || '/users/' || account.username
                   AND account.id_scheme IS DISTINCT FROM 1)
               OR (audience.uri = $2 || '/ap/users/' || account.id::text
                   AND account.id_scheme = 1)
             )
          ORDER BY audience.position, account.id LIMIT 1",
    )
    .bind(&audience)
    .bind(origin.as_str().trim_end_matches('/'))
    .fetch_optional(pool)
    .await
    .map_err(|_| HandlerFailure::retry("remote Create audience signer lookup failed"))?;
    let follower_account_id = sqlx::query_scalar::<_, i64>(
        "SELECT follower.id
           FROM follows follow
           JOIN accounts follower ON follower.id = follow.account_id
          WHERE follow.target_account_id = $1 AND follower.domain IS NULL
            AND follower.private_key IS NOT NULL AND follower.private_key <> ''
          ORDER BY follow.id LIMIT 1",
    )
    .bind(source_account_id)
    .fetch_optional(pool)
    .await
    .map_err(|_| HandlerFailure::retry("remote Create follower signer lookup failed"))?;
    let signer_account_id = preferred_note_fetch_signer_id(
        valid_delivery_target,
        addressed_account_id,
        follower_account_id,
    )
    .unwrap_or(-99);
    Repository::from_pool(pool.clone())
        .account(signer_account_id)
        .await
        .map_err(|_| HandlerFailure::retry("remote Create signer account lookup failed"))?
        .filter(|account| {
            account
                .private_key
                .as_ref()
                .is_some_and(crate::mastodon::SecretText::is_present)
        })
        .ok_or_else(|| HandlerFailure::permanent("remote Create signer is unavailable"))
}

pub(super) fn remote_announce_audience(
    arguments: &Value,
    field: &str,
) -> Result<Vec<String>, HandlerFailure> {
    let Some(value) = arguments.get(field) else {
        return Ok(Vec::new());
    };
    let values: Vec<&Value> = match value {
        Value::Null => Vec::new(),
        Value::Array(values) => values.iter().collect(),
        value => vec![value],
    };
    if values.len() > 100 {
        return Err(HandlerFailure::permanent(
            "remote Announce resolution audience is too large",
        ));
    }
    values
        .iter()
        .map(|value| {
            remote_uri_value(Some(value))
                .filter(|value| !value.trim().is_empty())
                .map(ToOwned::to_owned)
                .ok_or_else(|| {
                    HandlerFailure::permanent(
                        "remote Announce resolution audience contains an invalid URI",
                    )
                })
        })
        .collect()
}

// A fetched wrapper is authenticated by its transport origin, not its claimed actor.
// Check this before allowing any embedded object to reach a writer.
pub(super) fn validate_fetched_activity_actor(
    activity_uri: &str,
    actor_uri: &str,
) -> Result<(), HandlerFailure> {
    let matches_origin = Url::parse(activity_uri)
        .ok()
        .zip(Url::parse(actor_uri).ok())
        .is_some_and(|(activity, actor)| same_url_origin(&activity, &actor));
    if !matches_origin {
        return Err(HandlerFailure::permanent(
            "remote fetched activity actor does not match the requested origin",
        ));
    }
    Ok(())
}

pub(super) fn remote_note_document(
    document: &Value,
    object_uri: &str,
) -> Result<(Value, String, String), HandlerFailure> {
    let (object, actor_uri, target_uri) = if ["Note", "Question"]
        .into_iter()
        .any(|kind| equals_or_includes(document.get("type"), kind))
    {
        let actor_uri = remote_uri_value(document.get("attributedTo")).ok_or_else(|| {
            HandlerFailure::permanent("remote Announce target Note has no author")
        })?;
        if remote_uri_value(document.get("id")) != Some(object_uri) {
            return Err(HandlerFailure::permanent(
                "remote Announce target ID does not match the requested URI",
            ));
        }
        (document, actor_uri, object_uri)
    } else if equals_or_includes(document.get("type"), "Create") {
        if remote_uri_value(document.get("id")) != Some(object_uri) {
            return Err(HandlerFailure::permanent(
                "remote Announce target Create ID does not match the requested URI",
            ));
        }
        let object = document.get("object").ok_or_else(|| {
            HandlerFailure::permanent("remote Announce target Create has no object")
        })?;
        if !["Note", "Question"]
            .into_iter()
            .any(|kind| equals_or_includes(object.get("type"), kind))
        {
            return Err(HandlerFailure::permanent(
                "remote Announce target Create object is not a Note or Question",
            ));
        }
        let actor_uri = remote_uri_value(document.get("actor")).ok_or_else(|| {
            HandlerFailure::permanent("remote Announce target Create has no actor")
        })?;
        validate_fetched_activity_actor(object_uri, actor_uri)?;
        if remote_uri_value(object.get("attributedTo")) != Some(actor_uri) {
            return Err(HandlerFailure::permanent(
                "remote Announce target Note author does not match Create actor",
            ));
        }
        let target_uri = remote_uri_value(object.get("id"))
            .ok_or_else(|| HandlerFailure::permanent("remote Announce target Note has no ID"))?;
        (object, actor_uri, target_uri)
    } else {
        return Err(HandlerFailure::permanent(
            "remote Announce target is not a Note, Question, or Create",
        ));
    };
    let object_map = object
        .as_object()
        .ok_or_else(|| HandlerFailure::permanent("remote Announce target Note is not an object"))?;
    validate_note_object(actor_uri, object_map)
        .map_err(|_| HandlerFailure::permanent("remote Announce target Note is invalid"))?;
    Ok((object.clone(), actor_uri.to_owned(), target_uri.to_owned()))
}

#[derive(Debug)]
pub(super) enum RemoteAnnounceTarget {
    Note {
        object: Value,
        actor_uri: String,
        status_uri: String,
    },
    Announce {
        activity_uri: String,
        actor_uri: String,
        object_uri: String,
        embedded_note: Option<Value>,
        to: Vec<String>,
        cc: Vec<String>,
        published_at: Option<String>,
    },
}

pub(super) fn remote_announce_document(
    document: &Value,
    object_uri: &str,
) -> Result<RemoteAnnounceTarget, HandlerFailure> {
    if !equals_or_includes(document.get("type"), "Announce") {
        let (object, actor_uri, status_uri) = remote_note_document(document, object_uri)?;
        return Ok(RemoteAnnounceTarget::Note {
            object,
            actor_uri,
            status_uri,
        });
    }
    let activity_uri = remote_uri_value(document.get("id"))
        .ok_or_else(|| HandlerFailure::permanent("remote nested Announce has no activity ID"))?;
    if activity_uri != object_uri {
        return Err(HandlerFailure::permanent(
            "remote nested Announce ID does not match the requested URI",
        ));
    }
    let actor_uri = remote_uri_value(document.get("actor"))
        .ok_or_else(|| HandlerFailure::permanent("remote nested Announce has no actor"))?;
    validate_fetched_activity_actor(object_uri, actor_uri)?;
    let nested_object = document
        .get("object")
        .ok_or_else(|| HandlerFailure::permanent("remote nested Announce has no object"))?;
    let nested_object_uri = remote_uri_value(Some(nested_object))
        .ok_or_else(|| HandlerFailure::permanent("remote nested Announce object has no URI"))?;
    let embedded_note = nested_object.as_object().and_then(|object| {
        ["Note", "Question"]
            .into_iter()
            .any(|kind| equals_or_includes(object.get("type"), kind))
            .then(|| {
                let note_actor_uri = remote_uri_value(object.get("attributedTo"))?;
                // Only self-boosts inherit the wrapper's authority. Foreign authors must
                // be resolved through their canonical object URI, even on the same server.
                (note_actor_uri == actor_uri
                    && remote_uri_value(object.get("id")) == Some(nested_object_uri)
                    && validate_note_object(note_actor_uri, object).is_ok())
                .then(|| nested_object.clone())
            })?
    });
    Ok(RemoteAnnounceTarget::Announce {
        activity_uri: activity_uri.to_owned(),
        actor_uri: actor_uri.to_owned(),
        object_uri: nested_object_uri.to_owned(),
        embedded_note,
        to: remote_announce_audience(document, "to")?,
        cc: remote_announce_audience(document, "cc")?,
        published_at: document
            .get("published")
            .map(|value| {
                value.as_str().map(ToOwned::to_owned).ok_or_else(|| {
                    HandlerFailure::permanent("remote nested Announce timestamp is invalid")
                })
            })
            .transpose()?,
    })
}

pub(super) async fn fetch_remote_announce_target(
    config: &ActivityPubDeliveryConfig,
    fetcher: &RemoteFetcher,
    object_url: Url,
    signer: &HttpSignatureSigner<'_>,
) -> Result<RemoteAnnounceTarget, HandlerFailure> {
    #[cfg(not(feature = "test-support"))]
    let _ = config;
    let response = {
        #[cfg(feature = "test-support")]
        if let Some(endpoint) = config.remote_fetch_endpoint {
            fetcher
                .get_for_test_endpoint(
                    object_url.clone(),
                    THREAD_ACTIVITYPUB_CONTENT_TYPES,
                    endpoint,
                )
                .await
        } else {
            fetcher
                .get_signed(object_url.clone(), THREAD_ACTIVITYPUB_CONTENT_TYPES, signer)
                .await
        }
        #[cfg(not(feature = "test-support"))]
        {
            fetcher
                .get_signed(object_url.clone(), THREAD_ACTIVITYPUB_CONTENT_TYPES, signer)
                .await
        }
    }
    .map_err(|error| remote_announce_fetch_failure(&error))?;
    if !same_url_origin(&response.url, &object_url) {
        return Err(HandlerFailure::permanent(
            "remote Announce target redirected to another origin",
        ));
    }
    let document = serde_json::from_slice::<Value>(&response.body)
        .map_err(|_| HandlerFailure::permanent("remote Announce target JSON is invalid"))?;
    remote_announce_document(&document, object_url.as_str())
}

pub(super) async fn resolve_remote_note_author(
    pool: &PgPool,
    config: &ActivityPubDeliveryConfig,
    writer: &WriteRepository,
    fetcher: &RemoteFetcher,
    actor_uri: &str,
    signer: &HttpSignatureSigner<'_>,
) -> Result<i64, HandlerFailure> {
    if let Some(account_id) = sqlx::query_scalar::<_, i64>(
        "SELECT id FROM accounts WHERE uri = $1 AND domain IS NOT NULL ORDER BY id LIMIT 1",
    )
    .bind(actor_uri)
    .fetch_optional(pool)
    .await
    .map_err(|_| HandlerFailure::retry("remote Announce target author lookup failed"))?
    {
        return Ok(account_id);
    }
    let parsed_actor_url = Url::parse(actor_uri)
        .map_err(|_| HandlerFailure::permanent("remote Announce target author URI is invalid"))?;
    let actor_domain = inbox_actor_domain(actor_uri).ok_or_else(|| {
        HandlerFailure::permanent("remote Announce target author has no valid domain")
    })?;
    if same_url_origin(&parsed_actor_url, &config.origin) {
        return Err(HandlerFailure::permanent(
            "remote Announce target author is local",
        ));
    }
    if !Repository::from_pool(pool.clone())
        .remote_domain_allowed(&actor_domain, config.limited_federation)
        .await
        .map_err(|_| HandlerFailure::retry("remote Announce target author policy lookup failed"))?
    {
        return Err(HandlerFailure::permanent(
            "remote Announce target author domain is not allowed",
        ));
    }
    let actor = RemoteAccountResolver::new(fetcher.clone())
        .resolve_actor_uri_with_signer(&parsed_actor_url, Some(signer))
        .await
        .map_err(|error| remote_announce_fetch_failure(&error))?;
    writer
        .upsert_remote_actor(
            &actor.username,
            &actor_domain,
            config.limited_federation,
            &actor,
        )
        .await
        .map_err(|error| remote_thread_write_failure(&error))
}

pub(super) async fn resolve_announce_fetch_signer(
    pool: &PgPool,
    source_account_id: i64,
    delivery_target_account_id: Option<i64>,
) -> Result<Account, HandlerFailure> {
    let repository = Repository::from_pool(pool.clone());
    let preferred_account_id = if let Some(delivery_target_account_id) = delivery_target_account_id
    {
        sqlx::query_scalar::<_, i64>(
            "SELECT id FROM accounts
              WHERE id = $1 AND domain IS NULL AND private_key IS NOT NULL
                AND private_key <> ''",
        )
        .bind(delivery_target_account_id)
        .fetch_optional(pool)
        .await
        .map_err(|_| HandlerFailure::retry("remote Announce signer lookup failed"))?
    } else {
        None
    };
    let follower_account_id = sqlx::query_scalar::<_, i64>(
        "SELECT follower.id
               FROM follows follow
               JOIN accounts follower ON follower.id = follow.account_id
              WHERE follow.target_account_id = $1 AND follower.domain IS NULL
                AND follower.private_key IS NOT NULL AND follower.private_key <> ''
              ORDER BY follow.id DESC
              LIMIT 1",
    )
    .bind(source_account_id)
    .fetch_optional(pool)
    .await
    .map_err(|_| HandlerFailure::retry("remote Announce follower signer lookup failed"))?;
    let signer_account_id = follower_account_id.or(preferred_account_id);
    if let Some(signer_account_id) = signer_account_id
        && let Some(account) = repository
            .account(signer_account_id)
            .await
            .map_err(|_| HandlerFailure::retry("remote Announce signer account lookup failed"))?
        && account
            .private_key
            .as_ref()
            .is_some_and(crate::mastodon::SecretText::is_present)
    {
        return Ok(account);
    }
    repository
        .account(-99)
        .await
        .map_err(|_| HandlerFailure::retry("remote Announce instance lookup failed"))?
        .ok_or_else(|| HandlerFailure::permanent("remote Announce instance actor is missing"))
}

pub(super) const MAX_REMOTE_ANNOUNCE_RESOLUTION_DEPTH: u8 = 4;

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(super) async fn materialize_remote_announce_target(
    pool: &PgPool,
    config: &ActivityPubDeliveryConfig,
    writer: &WriteRepository,
    fetcher: &RemoteFetcher,
    object_uri: &str,
    embedded_note: Option<Value>,
    signer: &HttpSignatureSigner<'_>,
    delivery_target_account_id: Option<i64>,
    depth: u8,
) -> Result<Option<String>, HandlerFailure> {
    if depth > MAX_REMOTE_ANNOUNCE_RESOLUTION_DEPTH {
        return Err(HandlerFailure::permanent(
            "remote Announce target nesting is too deep",
        ));
    }
    if writer
        .remote_announce_target_exists(object_uri, config.origin.as_str())
        .await
        .map_err(|error| {
            remote_note_write_failure(&error, "remote Announce target lookup failed")
        })?
    {
        return Ok(Some(object_uri.to_owned()));
    }
    let target_url = Url::parse(object_uri)
        .map_err(|_| HandlerFailure::permanent("remote Announce target URI is invalid"))?;
    if same_url_origin(&target_url, &config.origin) {
        return Ok(None);
    }
    let object_domain = inbox_actor_domain(object_uri).ok_or_else(|| {
        HandlerFailure::permanent("remote Announce target URI has no valid domain")
    })?;
    if !Repository::from_pool(pool.clone())
        .remote_domain_allowed(&object_domain, config.limited_federation)
        .await
        .map_err(|_| HandlerFailure::retry("remote Announce target policy lookup failed"))?
    {
        return Err(HandlerFailure::permanent(
            "remote Announce target domain is not allowed",
        ));
    }
    let target = if let Some(note) = embedded_note {
        let (object, actor_uri, status_uri) = remote_note_document(&note, object_uri)?;
        RemoteAnnounceTarget::Note {
            object,
            actor_uri,
            status_uri,
        }
    } else {
        fetch_remote_announce_target(config, fetcher, target_url, signer).await?
    };
    match target {
        RemoteAnnounceTarget::Note {
            object,
            actor_uri,
            status_uri,
        } => {
            let account_id =
                resolve_remote_note_author(pool, config, writer, fetcher, &actor_uri, signer)
                    .await?;
            let written = writer
                .apply_remote_note_create(
                    account_id,
                    &actor_uri,
                    &object,
                    delivery_target_account_id,
                    config.origin.as_str(),
                )
                .await
                .map_err(|error| {
                    remote_note_write_failure(&error, "fetched remote Announce Note write failed")
                })?;
            Ok(written.map(|_| status_uri))
        }
        RemoteAnnounceTarget::Announce {
            activity_uri,
            actor_uri,
            object_uri: nested_object_uri,
            embedded_note,
            to,
            cc,
            published_at,
        } => {
            let Some(nested_status_uri) = Box::pin(materialize_remote_announce_target(
                pool,
                config,
                writer,
                fetcher,
                &nested_object_uri,
                embedded_note,
                signer,
                delivery_target_account_id,
                depth + 1,
            ))
            .await?
            else {
                return Ok(None);
            };
            let account_id =
                resolve_remote_note_author(pool, config, writer, fetcher, &actor_uri, signer)
                    .await?;
            let written = writer
                .apply_remote_announce(
                    account_id,
                    &actor_uri,
                    &activity_uri,
                    &nested_status_uri,
                    &to,
                    &cc,
                    published_at.as_deref(),
                    config.origin.as_str(),
                )
                .await
                .map_err(|error| {
                    remote_note_write_failure(&error, "nested remote Announce write failed")
                })?;
            Ok(written.map(|_| activity_uri))
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn schedule_remote_announce_resolution(
    pool: &PgPool,
    source_account_id: i64,
    actor_uri: &str,
    activity_uri: &str,
    target_uri: &str,
    to: &[String],
    cc: &[String],
    published_at: Option<&str>,
    delivery_target_account_id: Option<i64>,
) -> Result<(), HandlerFailure> {
    let job = JobSpec::new(
        Lane::Pull,
        ACTIVITYPUB_ANNOUNCE_RESOLVE_JOB_KIND,
        json!({
            "source_account_id": source_account_id,
            "actor_uri": actor_uri,
            "activity_uri": activity_uri,
            "object_uri": target_uri,
            "to": to,
            "cc": cc,
            "published_at": published_at,
            "delivery_target_account_id": delivery_target_account_id
        }),
    )
    .logical_key(announce_resolution_logical_key(activity_uri))
    .max_attempts(4);
    let mut transaction = pool.begin().await.map_err(|_| {
        HandlerFailure::retry("remote Announce resolution outbox transaction failed")
    })?;
    record_outbox_once_in(&mut transaction, &job)
        .await
        .map_err(|_| HandlerFailure::retry("remote Announce resolution outbox write failed"))?;
    transaction
        .commit()
        .await
        .map_err(|_| HandlerFailure::retry("remote Announce resolution outbox commit failed"))?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn process_remote_announce(
    pool: &PgPool,
    config: &ActivityPubDeliveryConfig,
    writer: &WriteRepository,
    source_account_id: i64,
    actor_uri: &str,
    activity_uri: &str,
    target_uri: &str,
    embedded_note: Option<Value>,
    to: &[String],
    cc: &[String],
    published_at: Option<&str>,
    delivery_target_account_id: Option<i64>,
) -> Result<(), HandlerFailure> {
    if writer
        .remote_announce_is_tombstoned(source_account_id, activity_uri)
        .await
        .map_err(|error| {
            remote_note_write_failure(&error, "remote Announce tombstone lookup failed")
        })?
    {
        return Ok(());
    }
    if !writer
        .remote_announce_is_relevant(source_account_id, delivery_target_account_id)
        .await
        .map_err(|error| {
            remote_note_write_failure(&error, "remote Announce relevance check failed")
        })?
    {
        return Ok(());
    }
    if writer
        .remote_announce_target_exists(target_uri, config.origin.as_str())
        .await
        .map_err(|error| {
            remote_note_write_failure(&error, "remote Announce target lookup failed")
        })?
    {
        writer
            .apply_remote_announce(
                source_account_id,
                actor_uri,
                activity_uri,
                target_uri,
                to,
                cc,
                published_at,
                config.origin.as_str(),
            )
            .await
            .map_err(|error| remote_note_write_failure(&error, "remote Announce write failed"))?;
        return Ok(());
    }
    let parsed_target = Url::parse(target_uri)
        .map_err(|_| HandlerFailure::permanent("remote Announce target URI is invalid"))?;
    if same_url_origin(&parsed_target, &config.origin) {
        return Ok(());
    }
    if let Some(note) = embedded_note
        && remote_uri_value(note.get("attributedTo")) == Some(actor_uri)
    {
        writer
            .apply_remote_note_create(
                source_account_id,
                actor_uri,
                &note,
                delivery_target_account_id,
                config.origin.as_str(),
            )
            .await
            .map_err(|error| {
                remote_note_write_failure(&error, "embedded remote Announce Note write failed")
            })?;
    } else {
        schedule_remote_announce_resolution(
            pool,
            source_account_id,
            actor_uri,
            activity_uri,
            target_uri,
            to,
            cc,
            published_at,
            delivery_target_account_id,
        )
        .await?;
        return Ok(());
    }
    writer
        .apply_remote_announce(
            source_account_id,
            actor_uri,
            activity_uri,
            target_uri,
            to,
            cc,
            published_at,
            config.origin.as_str(),
        )
        .await
        .map_err(|error| remote_note_write_failure(&error, "remote Announce write failed"))?;
    Ok(())
}

#[allow(clippy::too_many_lines)]
pub(super) async fn process_activitypub_announce_resolution(
    pool: PgPool,
    config: &ActivityPubDeliveryConfig,
    fetcher: &RemoteFetcher,
    arguments: &Value,
) -> Result<(), HandlerFailure> {
    let source_account_id = arguments
        .get("source_account_id")
        .and_then(Value::as_i64)
        .ok_or_else(|| {
            HandlerFailure::permanent("Announce resolution job is missing its source")
        })?;
    let actor_uri = arguments
        .get("actor_uri")
        .and_then(Value::as_str)
        .ok_or_else(|| HandlerFailure::permanent("Announce resolution job is missing its actor"))?;
    let activity_uri = arguments
        .get("activity_uri")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            HandlerFailure::permanent("Announce resolution job is missing its activity")
        })?;
    let object_uri = arguments
        .get("object_uri")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            HandlerFailure::permanent("Announce resolution job is missing its object")
        })?;
    let to = remote_announce_audience(arguments, "to")?;
    let cc = remote_announce_audience(arguments, "cc")?;
    let published_at = arguments.get("published_at").and_then(Value::as_str);
    let delivery_target_account_id = arguments
        .get("delivery_target_account_id")
        .and_then(Value::as_i64);
    let writer = WriteRepository::from_pool(pool.clone());
    if writer
        .remote_announce_is_tombstoned(source_account_id, activity_uri)
        .await
        .map_err(|error| {
            remote_note_write_failure(&error, "remote Announce tombstone lookup failed")
        })?
        || !writer
            .remote_announce_is_relevant(source_account_id, delivery_target_account_id)
            .await
            .map_err(|error| {
                remote_note_write_failure(&error, "remote Announce relevance check failed")
            })?
    {
        return Ok(());
    }
    if writer
        .remote_announce_target_exists(object_uri, config.origin.as_str())
        .await
        .map_err(|error| {
            remote_note_write_failure(&error, "remote Announce target lookup failed")
        })?
    {
        writer
            .apply_remote_announce(
                source_account_id,
                actor_uri,
                activity_uri,
                object_uri,
                &to,
                &cc,
                published_at,
                config.origin.as_str(),
            )
            .await
            .map_err(|error| remote_note_write_failure(&error, "remote Announce write failed"))?;
        return Ok(());
    }
    let signer_account =
        resolve_announce_fetch_signer(&pool, source_account_id, delivery_target_account_id).await?;
    let private_key = signer_account
        .private_key
        .as_ref()
        .filter(|key| key.is_present())
        .ok_or_else(|| HandlerFailure::permanent("remote Announce signer has no private key"))?;
    let signer_key_id = format!(
        "{}#main-key",
        activitypub::actor_url(&config.origin, &signer_account)
    );
    let signer = HttpSignatureSigner {
        key_id: &signer_key_id,
        private_key_pem: private_key.as_str(),
    };
    let Some(target_status_uri) = materialize_remote_announce_target(
        &pool,
        config,
        &writer,
        fetcher,
        object_uri,
        None,
        &signer,
        delivery_target_account_id,
        0,
    )
    .await?
    else {
        return Ok(());
    };
    writer
        .apply_remote_announce(
            source_account_id,
            actor_uri,
            activity_uri,
            &target_status_uri,
            &to,
            &cc,
            published_at,
            config.origin.as_str(),
        )
        .await
        .map_err(|error| remote_note_write_failure(&error, "remote Announce write failed"))?;
    Ok(())
}

#[allow(clippy::too_many_lines)]
pub(super) async fn process_activitypub_thread_resolution(
    pool: PgPool,
    config: &ActivityPubDeliveryConfig,
    fetcher: &RemoteFetcher,
    arguments: &Value,
) -> Result<(), HandlerFailure> {
    let child_status_id = arguments
        .get("child_status_id")
        .and_then(Value::as_i64)
        .ok_or_else(|| {
            HandlerFailure::permanent("thread resolution job is missing its child ID")
        })?;
    let parent_uri = arguments
        .get("parent_url")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            HandlerFailure::permanent("thread resolution job is missing its parent URL")
        })?;
    let parsed_parent_url = Url::parse(parent_uri)
        .map_err(|_| HandlerFailure::permanent("thread resolution parent URL is invalid"))?;
    let writer = WriteRepository::from_pool(pool.clone());
    if writer
        .resolve_remote_note_thread(child_status_id, parent_uri, config.origin.as_str())
        .await
        .map_err(|error| remote_thread_write_failure(&error))?
    {
        return Ok(());
    }
    if same_url_origin(&parsed_parent_url, &config.origin) {
        return Err(HandlerFailure::retry(
            "remote reply parent has not arrived locally",
        ));
    }
    let remote_domain = inbox_actor_domain(parent_uri).ok_or_else(|| {
        HandlerFailure::permanent("thread resolution parent URL has no valid remote domain")
    })?;
    if !Repository::from_pool(pool.clone())
        .remote_domain_allowed(&remote_domain, config.limited_federation)
        .await
        .map_err(|_| HandlerFailure::retry("thread resolution domain policy lookup failed"))?
    {
        return Err(HandlerFailure::permanent(
            "thread resolution parent domain is not allowed",
        ));
    }
    let instance = Repository::from_pool(pool.clone())
        .account(-99)
        .await
        .map_err(|_| HandlerFailure::retry("thread resolution instance lookup failed"))?
        .ok_or_else(|| HandlerFailure::permanent("thread resolution instance actor is missing"))?;
    let private_key = instance
        .private_key
        .as_ref()
        .filter(|key| key.is_present())
        .ok_or_else(|| {
            HandlerFailure::permanent("thread resolution instance has no private key")
        })?;
    let signer_key_id = format!(
        "{}#main-key",
        activitypub::actor_url(&config.origin, &instance)
    );
    let signer = HttpSignatureSigner {
        key_id: &signer_key_id,
        private_key_pem: private_key.as_str(),
    };
    let response = fetcher
        .get_signed(
            parsed_parent_url.clone(),
            THREAD_ACTIVITYPUB_CONTENT_TYPES,
            &signer,
        )
        .await
        .map_err(|error| remote_thread_fetch_failure(&error))?;
    if !same_url_origin(&response.url, &parsed_parent_url) {
        return Err(HandlerFailure::permanent(
            "thread resolution parent redirected to another origin",
        ));
    }
    let document = serde_json::from_slice::<Value>(&response.body)
        .map_err(|_| HandlerFailure::permanent("thread resolution parent JSON is invalid"))?;
    let (object, actor_uri) = if ["Note", "Question"]
        .into_iter()
        .any(|kind| equals_or_includes(document.get("type"), kind))
    {
        let actor_uri = remote_uri_value(document.get("attributedTo"))
            .ok_or_else(|| HandlerFailure::permanent("thread resolution Note has no author"))?;
        (&document, actor_uri)
    } else if equals_or_includes(document.get("type"), "Create") {
        let object = document
            .get("object")
            .ok_or_else(|| HandlerFailure::permanent("thread resolution Create has no object"))?;
        if !["Note", "Question"]
            .into_iter()
            .any(|kind| equals_or_includes(object.get("type"), kind))
        {
            return Err(HandlerFailure::permanent(
                "thread resolution Create object is not a Note or Question",
            ));
        }
        let actor_uri = remote_uri_value(document.get("actor"))
            .ok_or_else(|| HandlerFailure::permanent("thread resolution Create has no actor"))?;
        if remote_uri_value(object.get("attributedTo")) != Some(actor_uri) {
            return Err(HandlerFailure::permanent(
                "thread resolution Note author does not match Create actor",
            ));
        }
        (object, actor_uri)
    } else {
        return Err(HandlerFailure::permanent(
            "thread resolution parent is not a Note, Question, or Create",
        ));
    };
    if remote_uri_value(object.get("id")) != Some(parent_uri) {
        return Err(HandlerFailure::permanent(
            "thread resolution parent ID does not match the requested URI",
        ));
    }
    let parsed_actor_url = Url::parse(actor_uri)
        .map_err(|_| HandlerFailure::permanent("thread resolution actor URI is invalid"))?;
    let actor_domain = inbox_actor_domain(actor_uri).ok_or_else(|| {
        HandlerFailure::permanent("thread resolution actor URI has no valid domain")
    })?;
    if !Repository::from_pool(pool.clone())
        .remote_domain_allowed(&actor_domain, config.limited_federation)
        .await
        .map_err(|_| HandlerFailure::retry("thread resolution actor policy lookup failed"))?
    {
        return Err(HandlerFailure::permanent(
            "thread resolution actor domain is not allowed",
        ));
    }
    let actor_account_id = sqlx::query_scalar::<_, i64>(
        "SELECT id FROM accounts WHERE uri = $1 AND domain IS NOT NULL ORDER BY id LIMIT 1",
    )
    .bind(actor_uri)
    .fetch_optional(&pool)
    .await
    .map_err(|_| HandlerFailure::retry("thread resolution actor lookup failed"))?;
    let actor_account_id = if let Some(actor_account_id) = actor_account_id {
        actor_account_id
    } else {
        let actor = RemoteAccountResolver::new(fetcher.clone())
            .resolve_actor_uri_with_signer(&parsed_actor_url, Some(&signer))
            .await
            .map_err(|error| remote_thread_fetch_failure(&error))?;
        writer
            .upsert_remote_actor(
                &actor.username,
                &actor_domain,
                config.limited_federation,
                &actor,
            )
            .await
            .map_err(|error| remote_thread_write_failure(&error))?
    };
    writer
        .apply_remote_note_create(
            actor_account_id,
            actor_uri,
            object,
            None,
            config.origin.as_str(),
        )
        .await
        .map_err(|error| remote_thread_write_failure(&error))?;
    if writer
        .resolve_remote_note_thread(child_status_id, parent_uri, config.origin.as_str())
        .await
        .map_err(|error| remote_thread_write_failure(&error))?
    {
        Ok(())
    } else {
        Err(HandlerFailure::retry(
            "fetched remote reply parent was not persisted",
        ))
    }
}

#[allow(clippy::too_many_lines)]
pub(super) async fn process_activitypub_emoji(
    pool: PgPool,
    queue: Queue,
    config: &ActivityPubDeliveryConfig,
    fetcher: &RemoteFetcher,
    media_root: PaperclipRoot,
    arguments: &Value,
) -> Result<(), HandlerFailure> {
    let emoji_id = arguments
        .get("emoji_id")
        .and_then(Value::as_i64)
        .ok_or_else(|| HandlerFailure::permanent("emoji fetch job has no emoji ID"))?;
    let expected_url = arguments
        .get("remote_url")
        .and_then(Value::as_str)
        .ok_or_else(|| HandlerFailure::permanent("emoji fetch job has no URL"))?;
    let owner_domain = arguments
        .get("domain")
        .and_then(Value::as_str)
        .ok_or_else(|| HandlerFailure::permanent("emoji fetch job has no domain"))?;
    let advertised_type = arguments
        .get("media_type")
        .filter(|value| !value.is_null())
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| HandlerFailure::permanent("emoji media type is invalid"))
        })
        .transpose()?;
    let remote_url = Url::parse(expected_url)
        .map_err(|_| HandlerFailure::permanent("emoji fetch URL is invalid"))?;
    let image_domain = canonical_remote_domain_from_url(&remote_url)
        .map_err(|_| HandlerFailure::permanent("emoji fetch URL has no valid domain"))?;
    let repository = Repository::from_pool(pool.clone());
    for domain in [owner_domain, image_domain.as_str()] {
        if !repository
            .remote_media_allowed(domain, config.limited_federation)
            .await
            .map_err(|_| HandlerFailure::retry("emoji domain policy lookup failed"))?
        {
            return Err(HandlerFailure::permanent(
                "remote emoji domain is not allowed",
            ));
        }
    }
    let current = sqlx::query_as::<_, (String, String)>(
        "SELECT domain, image_remote_url FROM custom_emojis WHERE id = $1",
    )
    .bind(emoji_id)
    .fetch_optional(&pool)
    .await
    .map_err(|_| HandlerFailure::retry("remote emoji lookup failed"))?;
    let Some((domain, current_url)) = current else {
        return Ok(());
    };
    if domain != owner_domain || current_url != expected_url {
        return Ok(());
    }
    let fetcher = fetcher.with_limits(RemoteFetchLimits {
        max_response_bytes: 256 * 1024,
        ..RemoteFetchLimits::default()
    });
    #[cfg(feature = "test-support")]
    let response = match config.remote_media_endpoint {
        Some(endpoint) => {
            fetcher
                .get_for_test_endpoint(remote_url.clone(), REMOTE_MEDIA_CONTENT_TYPES, endpoint)
                .await
        }
        None => {
            fetcher
                .get(remote_url.clone(), REMOTE_MEDIA_CONTENT_TYPES)
                .await
        }
    };
    #[cfg(not(feature = "test-support"))]
    let response = fetcher
        .get(remote_url.clone(), REMOTE_MEDIA_CONTENT_TYPES)
        .await;
    let response = response.map_err(|error| remote_media_fetch_failure(&error))?;
    let final_image_domain = canonical_remote_domain_from_url(&response.url)
        .map_err(|_| HandlerFailure::permanent("emoji response URL has no valid domain"))?;
    if !repository
        .remote_media_allowed(&final_image_domain, config.limited_federation)
        .await
        .map_err(|_| HandlerFailure::retry("emoji response domain policy lookup failed"))?
    {
        return Err(HandlerFailure::permanent(
            "remote emoji response domain is not allowed",
        ));
    }
    let content_type = response
        .content_type
        .as_deref()
        .and_then(|value| value.split(';').next())
        .map(str::trim)
        .map(str::to_ascii_lowercase)
        .filter(|value| matches!(value.as_str(), "image/png" | "image/gif" | "image/webp"))
        .ok_or_else(|| HandlerFailure::permanent("remote emoji content type is invalid"))?;
    if advertised_type.is_some_and(|advertised| !advertised.eq_ignore_ascii_case(&content_type)) {
        return Err(HandlerFailure::permanent(
            "remote emoji content type does not match its metadata",
        ));
    }
    let source_name = remote_url
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .filter(|name| !name.is_empty())
        .unwrap_or("emoji");
    let prepared = prepare_custom_emoji(emoji_id, source_name, &content_type, &response.body)
        .map_err(|error| HandlerFailure::permanent(format!("remote emoji is invalid: {error}")))?;
    let metadata = PaperclipMetadata {
        attachment: PaperclipAttachment::CustomEmojiImage,
        id: emoji_id,
        remote: true,
        storage_schema_version: Some(1),
        file_name: prepared.file_name.clone(),
        content_type: Some(prepared.content_type.clone()),
        variant: None,
    };
    let mut transaction = pool
        .begin()
        .await
        .map_err(|_| HandlerFailure::retry("remote emoji transaction failed"))?;
    let current_metadata = sqlx::query_as::<_, (Option<String>, Option<String>, Option<i32>)>(
        "SELECT image_file_name, image_content_type, image_storage_schema_version
         FROM custom_emojis WHERE id = $1 AND domain = $2 AND image_remote_url = $3 FOR UPDATE",
    )
    .bind(emoji_id)
    .bind(owner_domain)
    .bind(expected_url)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|_| HandlerFailure::retry("remote emoji fence lookup failed"))?;
    let Some((old_file_name, old_content_type, old_storage_schema_version)) = current_metadata
    else {
        transaction
            .rollback()
            .await
            .map_err(|_| HandlerFailure::retry("remote emoji rollback failed"))?;
        return Ok(());
    };
    let writer = WriteRepository::from_pool(pool.clone());
    for domain in [owner_domain, final_image_domain.as_str()] {
        if !writer
            .remote_media_allowed_in_transaction(
                &mut transaction,
                domain,
                config.limited_federation,
            )
            .await
            .map_err(|_| HandlerFailure::retry("emoji install policy lookup failed"))?
        {
            return Err(HandlerFailure::permanent(
                "remote emoji domain became disallowed",
            ));
        }
    }
    let old_metadata = old_file_name.clone().map(|file_name| PaperclipMetadata {
        attachment: PaperclipAttachment::CustomEmojiImage,
        id: emoji_id,
        remote: true,
        storage_schema_version: old_storage_schema_version,
        file_name,
        content_type: old_content_type,
        variant: None,
    });
    let replacing = old_file_name.as_deref() != Some(prepared.file_name.as_str());
    let mut written_files = if replacing {
        let reconciliation_paths = old_metadata
            .iter()
            .chain(std::iter::once(&metadata))
            .flat_map(|metadata| {
                ["original", "static"]
                    .into_iter()
                    .filter_map(|style| metadata.relative_path(style))
            })
            .collect::<BTreeSet<_>>();
        queue
            .enqueue(&JobSpec::new(
                Lane::Maintenance,
                ACTIVITYPUB_EMOJI_CLEANUP_JOB_KIND,
                json!({"emoji_id": emoji_id, "paths": reconciliation_paths}),
            ))
            .await
            .map_err(|_| HandlerFailure::retry("emoji reconciliation could not be queued"))?;
        let paths = write_prepared_custom_emoji(&media_root, &metadata, &prepared)
            .map_err(|_| HandlerFailure::retry("remote emoji file write failed"))?;
        Some(WrittenMediaFiles::new(&media_root, paths))
    } else {
        None
    };
    let updated = sqlx::query(
        "UPDATE custom_emojis SET image_content_type = $3, image_file_name = $4,
             image_file_size = $5, image_storage_schema_version = 1,
             image_updated_at = clock_timestamp(), updated_at = clock_timestamp()
         WHERE id = $1 AND image_remote_url = $2",
    )
    .bind(emoji_id)
    .bind(expected_url)
    .bind(&prepared.content_type)
    .bind(&prepared.file_name)
    .bind(prepared.file_size)
    .execute(&mut *transaction)
    .await
    .map_err(|_| HandlerFailure::retry("remote emoji metadata update failed"))?;
    if updated.rows_affected() == 0 {
        if let Some(files) = &mut written_files {
            files.cleanup();
        }
        transaction
            .rollback()
            .await
            .map_err(|_| HandlerFailure::retry("remote emoji rollback failed"))?;
        return Ok(());
    }
    if let Some(files) = &mut written_files {
        files.disarm();
    }
    // A commit error is ambiguous. The reconciliation job keeps whichever file set the row names.
    #[cfg(feature = "test-support")]
    if media_root.take_commit_before_fault() {
        return Err(HandlerFailure::retry("remote emoji commit failed"));
    }
    transaction
        .commit()
        .await
        .map_err(|_| HandlerFailure::retry("remote emoji commit failed"))?;
    #[cfg(feature = "test-support")]
    if media_root.take_commit_after_fault() {
        return Err(HandlerFailure::retry("remote emoji commit failed"));
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
pub(super) async fn process_activitypub_media(
    pool: PgPool,
    config: &ActivityPubDeliveryConfig,
    fetcher: &RemoteFetcher,
    media_root: PaperclipRoot,
    arguments: &Value,
    attempt: i32,
    max_attempts: i32,
) -> Result<(), HandlerFailure> {
    let media_id = arguments
        .get("media_id")
        .and_then(Value::as_i64)
        .ok_or_else(|| HandlerFailure::permanent("media job is missing its media ID"))?;
    let media = sqlx::query_as::<
        _,
        (
            Option<i64>,
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
        ),
    >(
        "SELECT media.account_id, media.remote_url, media.file_file_name,
                media.file_content_type, media.blurhash, account.domain
           FROM media_attachments media
           JOIN accounts account ON account.id = media.account_id
           JOIN statuses status ON status.id = media.status_id
          WHERE media.id = $1 AND status.deleted_at IS NULL",
    )
    .bind(media_id)
    .fetch_optional(&pool)
    .await
    .map_err(|_| HandlerFailure::retry("remote media lookup failed"))?;
    let Some((
        Some(account_id),
        remote_url,
        existing_file_name,
        existing_content_type,
        blurhash,
        account_domain,
    )) = media
    else {
        return Ok(());
    };
    if remote_url.is_empty() || existing_file_name.is_some() {
        return Ok(());
    }
    let Ok(remote_url) = Url::parse(&remote_url) else {
        mark_remote_media_failed(&pool, media_id, &remote_url).await?;
        return Err(HandlerFailure::permanent("remote media URL is invalid"));
    };
    let account_domain = account_domain
        .ok_or_else(|| HandlerFailure::permanent("remote media account has no domain"))?;
    let remote_domain = canonical_remote_domain(&account_domain)
        .map_err(|_| HandlerFailure::permanent("remote media account has no valid domain"))?;
    if !Repository::from_pool(pool.clone())
        .remote_media_allowed(&remote_domain, config.limited_federation)
        .await
        .map_err(|_| HandlerFailure::retry("remote media domain policy lookup failed"))?
    {
        mark_remote_media_failed(&pool, media_id, remote_url.as_str()).await?;
        return Err(HandlerFailure::permanent(
            "remote media domain is not allowed",
        ));
    }
    if !claim_remote_media_processing(&pool, media_id, remote_url.as_str()).await? {
        return Ok(());
    }
    let file_name = remote_url
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .filter(|value| !value.is_empty())
        .unwrap_or("remote-media");
    #[cfg(feature = "test-support")]
    let fetcher = &fetcher
        .clone()
        .with_test_endpoint(config.remote_media_endpoint);
    let media_response = match RemoteMediaFetcher::new(fetcher, existing_content_type.as_deref()) {
        Ok(fetcher) => {
            fetcher
                .get_with_policy(remote_url.clone(), |url| {
                    let repository = Repository::from_pool(pool.clone());
                    async move {
                        let domain = canonical_remote_domain_from_url(&url)?;
                        if repository
                            .remote_media_allowed(&domain, config.limited_federation)
                            .await
                            .map_err(|_| RemoteFetchError::Request)?
                        {
                            Ok(())
                        } else {
                            Err(RemoteFetchError::PolicyDenied)
                        }
                    }
                })
                .await
        }
        Err(error) => Err(error),
    };
    let (response, visited) = match media_response {
        Ok(response) => response,
        Err(error) => {
            let failure = remote_media_fetch_failure(&error);
            if failure.disposition == FailureDisposition::Permanent {
                mark_remote_media_failed(&pool, media_id, remote_url.as_str()).await?;
            } else if attempt >= max_attempts {
                mark_remote_media_failed(&pool, media_id, remote_url.as_str()).await?;
                return Err(HandlerFailure::permanent(
                    "remote media retries were exhausted",
                ));
            } else {
                set_remote_media_processing(&pool, media_id, remote_url.as_str(), 0).await?;
            }
            return Err(failure);
        }
    };
    let Some(content_type) = response
        .content_type
        .as_deref()
        .and_then(|value| value.split(';').next())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase)
    else {
        mark_remote_media_failed(&pool, media_id, remote_url.as_str()).await?;
        return Err(HandlerFailure::permanent(
            "remote media content type is missing",
        ));
    };
    let prepared =
        match prepare_rich_media_attachment(account_id, file_name, &content_type, &response.body)
            .await
        {
            Ok(prepared) => prepared,
            Err(error) => {
                if matches!(
                    error,
                    crate::paperclip::MediaAttachmentError::UnsupportedContentType
                ) {
                    set_remote_media_processing(&pool, media_id, remote_url.as_str(), 2).await?;
                    return Ok(());
                }
                mark_remote_media_failed(&pool, media_id, remote_url.as_str()).await?;
                return Err(HandlerFailure::permanent(format!(
                    "remote media could not be processed: {error}"
                )));
            }
        };
    let visited_domains = visited
        .iter()
        .map(canonical_remote_domain_from_url)
        .collect::<Result<BTreeSet<_>, _>>()
        .map_err(|_| HandlerFailure::permanent("remote media hop has no valid domain"))?;
    let install_marker = RemoteMediaInstallMarker {
        file_name: prepared.file_name.clone(),
        content_type: prepared.content_type.clone(),
        file_size: prepared.file_size,
    };
    let transaction_install_marker = install_marker.clone();
    let metadata = PaperclipMetadata {
        attachment: PaperclipAttachment::MediaFile,
        id: media_id,
        remote: true,
        storage_schema_version: Some(1),
        file_name: prepared.file_name.clone(),
        content_type: Some(prepared.content_type.clone()),
        variant: None,
    };
    let writer = WriteRepository::from_pool(pool.clone());
    let persisted = writer
        .with_remote_domain_locks(&account_domain, || async {
            let mut transaction = pool.begin().await?;
            let mut pending_stream_events = Vec::new();
            let current = sqlx::query_as::<
                _,
                (
                    String,
                    Option<String>,
                    Option<String>,
                    bool,
                    Option<i64>,
                    Option<Value>,
                ),
            >(
                "SELECT media.remote_url, media.file_file_name, account.domain,
                        status.deleted_at IS NULL, media.account_id, media.file_meta
                   FROM media_attachments media
                   JOIN accounts account ON account.id = media.account_id
                   JOIN statuses status ON status.id = media.status_id
                  WHERE media.id = $1
                  FOR UPDATE OF media, account, status",
            )
            .bind(media_id)
            .fetch_optional(&mut *transaction)
            .await?;
            let Some((
                current_remote_url,
                current_file_name,
                current_domain,
                active,
                current_account_id,
                current_meta,
            )) = current
            else {
                transaction.rollback().await?;
                return Ok(false);
            };
            if !active
                || current_account_id != Some(account_id)
                || current_remote_url != remote_url.as_str()
                || current_domain.as_deref() != Some(account_domain.as_str())
                || current_file_name.is_some()
            {
                transaction.rollback().await?;
                return Ok(false);
            }
            let mut allowed = writer
                .remote_media_allowed_in_transaction(
                    &mut transaction,
                    &remote_domain,
                    config.limited_federation,
                )
                .await?;
            for domain in &visited_domains {
                allowed &= writer
                    .remote_media_allowed_in_transaction(
                        &mut transaction,
                        domain,
                        config.limited_federation,
                    )
                    .await?;
            }
            if !allowed {
                sqlx::query(
                    "UPDATE media_attachments SET processing = 3, updated_at = clock_timestamp()
                      WHERE id = $1 AND remote_url = $2",
                )
                .bind(media_id)
                .bind(remote_url.as_str())
                .execute(&mut *transaction)
                .await?;
                transaction.commit().await?;
                return Err(WriteError::Validation("remote media domain is not allowed"));
            }
            // Focus can be edited or cleared while fetching. Merge the current
            // locked row, never the prefetch snapshot, without replacing measured geometry.
            let mut file_meta = prepared.file_meta.clone();
            if let (Value::Object(target), Some(Value::Object(source))) =
                (&mut file_meta, current_meta)
            {
                for (key, value) in source {
                    if !matches!(key.as_str(), "original" | "small") {
                        target.insert(key, value);
                    }
                }
            }
            let written_paths = write_prepared_media(&media_root, &metadata, &prepared)?;
            let mut written_files = WrittenMediaFiles::new(&media_root, written_paths);
            let status_id = match sqlx::query_scalar::<_, i64>(
                "UPDATE media_attachments SET processing = 2, file_content_type = $3,
                    file_file_name = $4, file_file_size = $5, file_meta = $6::json,
                    file_storage_schema_version = 1, file_updated_at = clock_timestamp(),
                    blurhash = COALESCE($7, blurhash), type = $8, updated_at = clock_timestamp()
                  WHERE id = $1 AND status_id IS NOT NULL AND remote_url = $2
                    AND EXISTS (
                        SELECT 1 FROM statuses
                         WHERE statuses.id = media_attachments.status_id
                           AND statuses.deleted_at IS NULL
                    )
                  RETURNING status_id",
            )
            .bind(media_id)
            .bind(remote_url.as_str())
            .bind(&prepared.content_type)
            .bind(&prepared.file_name)
            .bind(prepared.file_size)
            .bind(file_meta)
            .bind(blurhash.or(prepared.blurhash))
            .bind(prepared.media_kind.database_type())
            .fetch_optional(&mut *transaction)
            .await
            {
                Ok(status_id) => status_id,
                Err(error) => {
                    written_files.cleanup();
                    let _ = transaction.rollback().await;
                    return Err(error.into());
                }
            };
            let Some(status_id) = status_id else {
                written_files.cleanup();
                transaction.commit().await?;
                return Ok(false);
            };
            WriteRepository::collect_remote_media_installed_stream_events_in(
                &mut transaction,
                &mut pending_stream_events,
                status_id,
                media_id,
            )
            .await?;
            // Until COMMIT is issued, cancellation must still remove the files.
            if let Err(error) =
                flush_stream_events_in(&mut transaction, &mut pending_stream_events).await
            {
                written_files.cleanup();
                let _ = transaction.rollback().await;
                return Err(error.into());
            }
            // A cancelled or failed COMMIT is ambiguous: PostgreSQL may still commit after this
            // future is dropped. Disarm only immediately before COMMIT; retain paths until the
            // synchronized probe below proves that the installation rolled back.
            #[cfg(feature = "test-support")]
            let (written_paths, commit_result) = if media_root.take_commit_before_fault() {
                transaction.rollback().await?;
                (
                    written_files.preserve(),
                    Err(sqlx::Error::Protocol(
                        "injected ambiguous metadata commit failure".to_owned(),
                    )),
                )
            } else {
                let written_paths = written_files.preserve();
                (written_paths, transaction.commit().await)
            };
            #[cfg(not(feature = "test-support"))]
            let (written_paths, commit_result) = {
                let written_paths = written_files.preserve();
                (written_paths, transaction.commit().await)
            };
            #[cfg(feature = "test-support")]
            let commit_result = if commit_result.is_ok() && media_root.take_commit_after_fault() {
                Err(sqlx::Error::Protocol(
                    "injected ambiguous metadata commit result".to_owned(),
                ))
            } else {
                commit_result
            };
            match commit_result {
                Ok(()) => Ok(true),
                Err(error) => {
                    match remote_media_install_committed(
                        &pool,
                        media_id,
                        remote_url.as_str(),
                        &transaction_install_marker,
                    )
                    .await
                    {
                        Ok(true) => Ok(true),
                        Ok(false) => {
                            WrittenMediaFiles::new(&media_root, written_paths).cleanup();
                            Err(error.into())
                        }
                        Err(_) => {
                            // A failed reconciliation leaves the commit genuinely ambiguous.
                            // Preserve files because deleting them could corrupt an installation
                            // that committed.
                            Err(error.into())
                        }
                    }
                }
            }
        })
        .await;
    match persisted {
        Ok(_) => Ok(()),
        Err(WriteError::Validation("remote media domain is not allowed")) => Err(
            HandlerFailure::permanent("remote media domain is not allowed"),
        ),
        Err(WriteError::Filesystem(_)) => {
            if attempt >= max_attempts {
                mark_remote_media_failed(&pool, media_id, remote_url.as_str()).await?;
                Err(HandlerFailure::permanent(
                    "remote media file retries were exhausted",
                ))
            } else {
                set_remote_media_processing(&pool, media_id, remote_url.as_str(), 0).await?;
                Err(HandlerFailure::retry("remote media file write failed"))
            }
        }
        Err(_) => {
            let committed = remote_media_install_committed(
                &pool,
                media_id,
                remote_url.as_str(),
                &install_marker,
            )
            .await
            .is_ok_and(|committed| committed);
            if committed {
                Ok(())
            } else if attempt >= max_attempts {
                mark_remote_media_failed(&pool, media_id, remote_url.as_str()).await?;
                Err(HandlerFailure::permanent(
                    "remote media metadata retries were exhausted",
                ))
            } else {
                set_remote_media_processing(&pool, media_id, remote_url.as_str(), 0).await?;
                Err(HandlerFailure::retry("remote media metadata update failed"))
            }
        }
    }
}

#[derive(Clone)]
pub(super) struct RemoteMediaInstallMarker {
    pub(super) file_name: String,
    pub(super) content_type: String,
    pub(super) file_size: i32,
}

pub(super) async fn remote_media_install_committed(
    pool: &PgPool,
    media_id: i64,
    remote_url: &str,
    marker: &RemoteMediaInstallMarker,
) -> Result<bool, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    let installed = sqlx::query_as::<
        _,
        (
            String,
            Option<i32>,
            Option<String>,
            Option<String>,
            Option<i32>,
            Option<i32>,
        ),
    >(
        "SELECT remote_url, processing, file_file_name, file_content_type,
                file_file_size, file_storage_schema_version
           FROM media_attachments WHERE id = $1 FOR UPDATE",
    )
    .bind(media_id)
    .fetch_optional(&mut *transaction)
    .await?;
    let committed = installed.is_some_and(
        |(current_url, processing, file_name, content_type, file_size, storage_schema_version)| {
            current_url == remote_url
                && processing == Some(2)
                && file_name.as_deref() == Some(marker.file_name.as_str())
                && content_type.as_deref() == Some(marker.content_type.as_str())
                && file_size == Some(marker.file_size)
                && storage_schema_version == Some(1)
        },
    );
    transaction.rollback().await?;
    Ok(committed)
}

pub(super) struct WrittenMediaFiles {
    pub(super) root: PaperclipRoot,
    pub(super) paths: Vec<String>,
}

impl WrittenMediaFiles {
    pub(super) fn new(root: &PaperclipRoot, paths: Vec<String>) -> Self {
        Self {
            root: root.clone(),
            paths,
        }
    }

    pub(super) fn cleanup(&mut self) {
        for path in self.paths.drain(..) {
            let _ = self.root.remove_file(Path::new(&path));
        }
    }

    pub(super) fn disarm(&mut self) {
        self.paths.clear();
    }

    pub(super) fn preserve(&mut self) -> Vec<String> {
        std::mem::take(&mut self.paths)
    }
}

impl Drop for WrittenMediaFiles {
    fn drop(&mut self) {
        self.cleanup();
    }
}

pub(super) async fn set_remote_media_processing(
    pool: &PgPool,
    media_id: i64,
    remote_url: &str,
    processing: i32,
) -> Result<bool, HandlerFailure> {
    let updated = sqlx::query(
        "UPDATE media_attachments SET processing = $3, updated_at = clock_timestamp()
          WHERE id = $1 AND status_id IS NOT NULL AND remote_url = $2
            AND file_file_name IS NULL
            AND EXISTS (
                SELECT 1 FROM statuses
                 WHERE statuses.id = media_attachments.status_id
                   AND statuses.deleted_at IS NULL
            )",
    )
    .bind(media_id)
    .bind(remote_url)
    .bind(processing)
    .execute(pool)
    .await
    .map_err(|_| HandlerFailure::retry("remote media processing state update failed"))?;
    Ok(updated.rows_affected() != 0)
}

pub(super) async fn claim_remote_media_processing(
    pool: &PgPool,
    media_id: i64,
    remote_url: &str,
) -> Result<bool, HandlerFailure> {
    // Lease fencing can cancel a handler after this state change, so retries must be able to
    // reclaim a still-unmaterialized attachment that is already marked as processing.
    let updated = sqlx::query(
        "UPDATE media_attachments SET processing = 1, updated_at = clock_timestamp()
          WHERE id = $1 AND status_id IS NOT NULL AND remote_url = $2
            AND file_file_name IS NULL
            AND EXISTS (
                SELECT 1 FROM statuses
                 WHERE statuses.id = media_attachments.status_id
                   AND statuses.deleted_at IS NULL
            )",
    )
    .bind(media_id)
    .bind(remote_url)
    .execute(pool)
    .await
    .map_err(|_| HandlerFailure::retry("remote media processing claim failed"))?;
    Ok(updated.rows_affected() != 0)
}

pub(super) async fn mark_remote_media_failed(
    pool: &PgPool,
    media_id: i64,
    remote_url: &str,
) -> Result<(), HandlerFailure> {
    set_remote_media_processing(pool, media_id, remote_url, 3)
        .await
        .map(|_| ())
}

pub(super) fn remote_media_fetch_failure(error: &RemoteFetchError) -> HandlerFailure {
    match error {
        RemoteFetchError::UnexpectedStatus(status)
            if *status != StatusCode::NOT_IMPLEMENTED
                && (*status == StatusCode::UNAUTHORIZED
                    || *status == StatusCode::REQUEST_TIMEOUT
                    || *status == StatusCode::TOO_MANY_REQUESTS
                    || status.is_server_error()) =>
        {
            HandlerFailure::retry(format!(
                "remote media fetch is temporarily unavailable: {error}"
            ))
        }
        RemoteFetchError::NoAddresses
        | RemoteFetchError::Dns
        | RemoteFetchError::Client
        | RemoteFetchError::Request
        | RemoteFetchError::BodyRead
        | RemoteFetchError::DomainBudgetExceeded => {
            HandlerFailure::retry(format!("remote media fetch failed: {error}"))
        }
        _ => HandlerFailure::permanent(format!("remote media is invalid: {error}")),
    }
}

pub(super) async fn schedule_remote_follow_accept(
    pool: &PgPool,
    repository: &Repository,
    config: &ActivityPubDeliveryConfig,
    source_account_id: i64,
    target_account_id: i64,
    follow_id: i64,
    follow_uri: &str,
) -> Result<(), HandlerFailure> {
    let source_account = repository
        .account(source_account_id)
        .await
        .map_err(|_| HandlerFailure::retry("remote Follow source lookup failed"))?
        .ok_or_else(|| HandlerFailure::permanent("remote Follow source account is missing"))?;
    let target_account = repository
        .account(target_account_id)
        .await
        .map_err(|_| HandlerFailure::retry("remote Follow target lookup failed"))?
        .ok_or_else(|| HandlerFailure::permanent("remote Follow target account is missing"))?;
    if source_account.domain.is_none() || target_account.domain.is_some() {
        return Err(HandlerFailure::permanent(
            "remote Follow acceptance accounts are not local/remote",
        ));
    }
    let remote_domain = source_account
        .domain
        .clone()
        .ok_or_else(|| HandlerFailure::permanent("remote Follow source has no domain"))?;
    let inbox_url = if source_account.inbox_url.is_empty() {
        source_account.shared_inbox_url.clone()
    } else {
        source_account.inbox_url.clone()
    };
    if inbox_url.is_empty() {
        return Err(HandlerFailure::permanent(
            "remote Follow source has no inbox",
        ));
    }
    let body = activitypub::accept(
        &config.origin,
        &target_account,
        follow_id,
        follow_uri,
        &source_account,
    );
    let delivery = JobSpec::new(
        Lane::Push,
        ACTIVITYPUB_DELIVERY_JOB_KIND,
        json!({
            "source_account_id": target_account_id,
            "inbox_url": inbox_url,
            "remote_domain": remote_domain,
            "body": body
        }),
    )
    .logical_key(activitypub::accept_delivery_logical_key(
        follow_id, follow_uri, &inbox_url,
    ));
    let mut transaction = pool
        .begin()
        .await
        .map_err(|_| HandlerFailure::retry("remote Follow acceptance outbox transaction failed"))?;
    let follow_exists = sqlx::query_scalar::<_, i64>(
        "SELECT id FROM follows
           WHERE id = $1 AND account_id = $2 AND target_account_id = $3 AND uri = $4
           FOR UPDATE",
    )
    .bind(follow_id)
    .bind(source_account_id)
    .bind(target_account_id)
    .bind(follow_uri)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|_| HandlerFailure::retry("remote Follow acceptance relationship lookup failed"))?
    .is_some();
    if !follow_exists {
        return Ok(());
    }
    record_outbox_once_in(&mut transaction, &delivery)
        .await
        .map_err(|_| HandlerFailure::retry("remote Follow acceptance outbox write failed"))?;
    transaction
        .commit()
        .await
        .map_err(|_| HandlerFailure::retry("remote Follow acceptance outbox commit failed"))?;
    Ok(())
}
