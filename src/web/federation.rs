//! ActivityPub federation handlers: inbox, actors, notes, collections and signatures.

use super::*;

pub(super) async fn federation_account(
    state: &WebState,
    account_id: i64,
    reject_suspension: bool,
) -> Result<Account, Response<Body>> {
    let account = match state.repository.account(account_id).await {
        Ok(Some(account)) => account,
        Ok(None) => return Err(not_found()),
        Err(_) => return Err(internal_error()),
    };
    if account.id != -99
        && (account.domain.is_some() || account.has_pending_user || account.has_unconfirmed_user)
    {
        return Err(not_found());
    }
    if reject_suspension && account.suspended_at.is_some() {
        let Ok(temporary) = state
            .repository
            .account_has_deletion_request(account.id)
            .await
        else {
            return Err(internal_error());
        };
        return Err(error_response(
            if temporary {
                StatusCode::FORBIDDEN
            } else {
                StatusCode::GONE
            },
            "Unavailable account",
        ));
    }
    Ok(account)
}

pub(super) async fn verify_optional_federation_signature(
    state: &WebState,
    client_ip: IpAddr,
    uri: &Uri,
    headers: &HeaderMap,
) -> Result<(), Response<Body>> {
    verify_federation_request(
        state,
        client_ip,
        &Method::GET,
        uri,
        headers,
        &[],
        false,
        true,
    )
    .await
    .map(|_| ())
}

#[derive(Clone, Debug)]
pub(super) struct VerifiedFederationRequest {
    key_id: String,
    actor_uri: Option<String>,
}

pub(super) async fn verify_required_federation_signature(
    state: &WebState,
    client_ip: IpAddr,
    method: &Method,
    uri: &Uri,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<VerifiedFederationRequest, Response<Body>> {
    verify_federation_request(state, client_ip, method, uri, headers, body, true, false)
        .await?
        .filter(|request| request.actor_uri.is_some())
        .ok_or_else(signature_verification_failure)
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(super) async fn verify_federation_request(
    state: &WebState,
    client_ip: IpAddr,
    method: &Method,
    uri: &Uri,
    headers: &HeaderMap,
    body: &[u8],
    required: bool,
    persist_refreshed_key: bool,
) -> Result<Option<VerifiedFederationRequest>, Response<Body>> {
    let Some(key_id) = signature_key_id(headers).map_err(|_| {
        error_response(
            StatusCode::UNAUTHORIZED,
            "Request signature verification failed",
        )
    })?
    else {
        if required || state.instance_runtime.limited_federation {
            return Err(error_response(
                StatusCode::UNAUTHORIZED,
                "Request signature verification failed",
            ));
        }
        return Ok(None);
    };
    let remote_domain = remote_signature_domain(&key_id, &state.local_domain, &state.origin)
        .map_err(|()| {
            error_response(
                StatusCode::UNAUTHORIZED,
                "Request signature verification failed",
            )
        })?;
    if let Some(domain) = &remote_domain {
        let allowed = state
            .repository
            .remote_domain_allowed(domain, state.instance_runtime.limited_federation)
            .await
            .map_err(|_| internal_error())?;
        if !allowed {
            return Err(error_response(
                StatusCode::FORBIDDEN,
                "Request signature verification failed",
            ));
        }
    }
    let key = state
        .repository
        .activitypub_signature_key(&key_id, state.origin.as_str())
        .await
        .map_err(|_| internal_error())?;
    if let Some(key) = key {
        match verify_federation_signature(
            method,
            uri,
            headers,
            &key.key_id,
            &key.public_key,
            key.revoked,
            key.expires_at,
            body,
        ) {
            Ok(()) => {
                let actor_uri = state
                    .repository
                    .account(key.account_id)
                    .await
                    .map_err(|_| internal_error())?
                    .map(|account| activitypub::actor_url(&state.origin, &account));
                return Ok(Some(VerifiedFederationRequest { key_id, actor_uri }));
            }
            Err(
                FederationSignatureError::Inactive
                | FederationSignatureError::Verification(
                    HttpSignatureError::OutsideTimeWindow
                    | HttpSignatureError::MissingDate
                    | HttpSignatureError::InvalidDate,
                ),
            ) => {
                return Err(signature_verification_failure());
            }
            Err(FederationSignatureError::Verification(error))
                if should_refresh_federation_key(error) && remote_domain.is_some() =>
            {
                return Ok(Some(
                    refresh_remote_federation_signature(
                        state,
                        client_ip,
                        method,
                        uri,
                        headers,
                        body,
                        &key_id,
                        persist_refreshed_key,
                    )
                    .await?,
                ));
            }
            Err(_) => return Err(signature_verification_failure()),
        }
    }
    if remote_domain.is_none() {
        return Err(error_response(
            StatusCode::UNAUTHORIZED,
            "Request signature verification failed",
        ));
    }
    let verified = refresh_remote_federation_signature(
        state,
        client_ip,
        method,
        uri,
        headers,
        body,
        &key_id,
        persist_refreshed_key,
    )
    .await?;
    Ok(Some(verified))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum FederationSignatureError {
    Inactive,
    Verification(HttpSignatureError),
}

#[allow(clippy::too_many_arguments)]
pub(super) fn verify_federation_signature(
    method: &Method,
    uri: &Uri,
    headers: &HeaderMap,
    key_id: &str,
    public_key: &str,
    revoked: bool,
    expires_at: Option<NaiveDateTime>,
    body: &[u8],
) -> Result<(), FederationSignatureError> {
    if revoked || expires_at.is_some_and(|expires_at| expires_at <= Utc::now().naive_utc()) {
        return Err(FederationSignatureError::Inactive);
    }
    let path_and_query = uri
        .path_and_query()
        .map_or_else(|| uri.path().to_owned(), |value| value.as_str().to_owned());
    let request = HttpSignatureRequest::new(method, &path_and_query, headers, body);
    let resolved_key = HttpSignatureKey {
        key_id,
        public_key_pem: public_key,
    };
    verify_http_signature(&request, &resolved_key, SystemTime::now())
        .map_err(FederationSignatureError::Verification)?;
    Ok(())
}

#[allow(clippy::result_large_err)]
#[allow(clippy::too_many_arguments)]
pub(super) fn verify_federation_signature_request(
    method: &Method,
    uri: &Uri,
    headers: &HeaderMap,
    key_id: &str,
    public_key: &str,
    revoked: bool,
    expires_at: Option<NaiveDateTime>,
    body: &[u8],
) -> Result<(), Response<Body>> {
    verify_federation_signature(
        method, uri, headers, key_id, public_key, revoked, expires_at, body,
    )
    .map_err(|_| signature_verification_failure())
}

pub(super) fn should_refresh_federation_key(error: HttpSignatureError) -> bool {
    matches!(
        error,
        HttpSignatureError::InvalidPublicKey | HttpSignatureError::SignatureMismatch
    )
}

pub(super) fn signature_verification_failure() -> Response<Body> {
    error_response(
        StatusCode::UNAUTHORIZED,
        "Request signature verification failed",
    )
}

pub(super) fn signature_fetch_circuit_open() -> Response<Body> {
    error_response(
        StatusCode::SERVICE_UNAVAILABLE,
        "Request signature verification failed",
    )
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(super) async fn refresh_remote_federation_signature(
    state: &WebState,
    client_ip: IpAddr,
    method: &Method,
    uri: &Uri,
    headers: &HeaderMap,
    body: &[u8],
    key_id: &str,
    persist_refreshed_key: bool,
) -> Result<VerifiedFederationRequest, Response<Body>> {
    let Some(instance) = state
        .repository
        .account(-99)
        .await
        .map_err(|_| internal_error())?
    else {
        return Err(signature_verification_failure());
    };
    let Some(private_key) = instance.private_key.as_ref().filter(|key| key.is_present()) else {
        return Err(signature_verification_failure());
    };
    let signer_key_id = format!(
        "{}#main-key",
        activitypub::actor_url(&state.origin, &instance)
    );
    let signer = HttpSignatureSigner {
        key_id: &signer_key_id,
        private_key_pem: private_key.as_str(),
    };
    let circuit_key = signature_fetch_circuit_key(client_ip);
    let circuit_open = match state.shared_rate_limiter.as_ref() {
        Some(shared) => shared.circuit_open(&circuit_key).await.unwrap_or(true),
        None => !state.signature_fetch_circuit.allow(client_ip),
    };
    if circuit_open {
        return Err(signature_fetch_circuit_open());
    }
    let resolution = match state
        .remote_account_resolver
        .resolve_key(key_id, Some(&signer))
        .await
    {
        Ok(resolution) => resolution,
        Err(error) => {
            if remote_signature_fetch_should_trip(&error) {
                match state.shared_rate_limiter.as_ref() {
                    Some(shared) => {
                        if shared
                            .record_circuit_failure(&circuit_key, SIGNATURE_FETCH_COOL_OFF)
                            .await
                            .is_err()
                        {
                            return Err(signature_fetch_circuit_open());
                        }
                    }
                    None => state.signature_fetch_circuit.record_failure(client_ip),
                }
            }
            return Err(remote_signature_fetch_failure(&error));
        }
    };
    let allowed = state
        .repository
        .remote_domain_allowed(
            &resolution.domain,
            state.instance_runtime.limited_federation,
        )
        .await
        .map_err(|_| internal_error())?;
    if !allowed {
        return Err(error_response(
            StatusCode::FORBIDDEN,
            "Request signature verification failed",
        ));
    }
    verify_federation_signature(
        method,
        uri,
        headers,
        key_id,
        &resolution.key.pem,
        false,
        None,
        body,
    )
    .map_err(|_| signature_verification_failure())?;
    let actor_uri = resolution.actor.id.to_string();
    if !persist_refreshed_key {
        return Ok(VerifiedFederationRequest {
            key_id: key_id.to_owned(),
            actor_uri: Some(actor_uri),
        });
    }
    let Some(writer) = state.write_repository.as_ref() else {
        return Ok(VerifiedFederationRequest {
            key_id: key_id.to_owned(),
            actor_uri: Some(actor_uri),
        });
    };
    let mut actor = resolution.actor.clone();
    if !actor
        .public_keys
        .iter()
        .any(|key| key.id == resolution.key.id)
    {
        actor.public_keys.push(resolution.key.clone());
    }
    writer
        .upsert_remote_actor(
            &actor.username,
            &resolution.domain,
            state.instance_runtime.limited_federation,
            &actor,
        )
        .await
        .map_err(|_| internal_error())?;
    let key = state
        .repository
        .activitypub_signature_key(key_id, state.origin.as_str())
        .await
        .map_err(|_| internal_error())?
        .ok_or_else(signature_verification_failure)?;
    verify_federation_signature_request(
        method,
        uri,
        headers,
        &key.key_id,
        &key.public_key,
        key.revoked,
        key.expires_at,
        body,
    )?;
    Ok(VerifiedFederationRequest {
        key_id: key_id.to_owned(),
        actor_uri: Some(actor_uri),
    })
}

#[derive(Clone, Copy, Debug)]
pub(super) enum InboxDeliveryTarget {
    Shared,
    Instance,
    Account(i64),
}

impl InboxDeliveryTarget {
    const fn account_id(self) -> Option<i64> {
        match self {
            Self::Shared => None,
            Self::Instance => Some(-99),
            Self::Account(account_id) => Some(account_id),
        }
    }
}

pub(super) async fn federation_inbox_shared(
    State(state): State<WebState>,
    Extension(metadata): Extension<RequestMetadata>,
    request: Request,
) -> Response<Body> {
    federation_inbox_request(
        state,
        request,
        InboxDeliveryTarget::Shared,
        metadata.client_ip,
    )
    .await
}

pub(super) async fn federation_inbox_instance(
    State(state): State<WebState>,
    Extension(metadata): Extension<RequestMetadata>,
    request: Request,
) -> Response<Body> {
    federation_inbox_request(
        state,
        request,
        InboxDeliveryTarget::Instance,
        metadata.client_ip,
    )
    .await
}

pub(super) async fn federation_inbox_username(
    State(state): State<WebState>,
    Extension(metadata): Extension<RequestMetadata>,
    Path(username): Path<String>,
    request: Request,
) -> Response<Body> {
    let account_id = match federation_local_account_id(&state, &username).await {
        Ok(account_id) => account_id,
        Err(response) => return response,
    };
    federation_inbox_request(
        state,
        request,
        InboxDeliveryTarget::Account(account_id),
        metadata.client_ip,
    )
    .await
}

pub(super) async fn federation_inbox_id(
    State(state): State<WebState>,
    Extension(metadata): Extension<RequestMetadata>,
    Path(id): Path<String>,
    request: Request,
) -> Response<Body> {
    let Some(account_id) = activitypub_path_id(&id) else {
        return not_found();
    };
    if let Err(response) = federation_account(&state, account_id, false).await {
        return response;
    }
    federation_inbox_request(
        state,
        request,
        InboxDeliveryTarget::Account(account_id),
        metadata.client_ip,
    )
    .await
}

#[allow(clippy::too_many_lines)]
pub(super) async fn federation_inbox_request(
    state: WebState,
    request: Request,
    target: InboxDeliveryTarget,
    client_ip: IpAddr,
) -> Response<Body> {
    let Some(queue) = state.queue.as_ref() else {
        return error_response(StatusCode::SERVICE_UNAVAILABLE, "Inbox unavailable");
    };
    if let Err(limited) = state
        .activitypub_inbox_limiter
        .check_shared(state.shared_rate_limiter.as_ref(), client_ip)
        .await
    {
        return rate_limited_response(limited);
    }
    if request
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .is_some_and(|length| length > ACTIVITYPUB_INBOX_BODY_LIMIT_BYTES as u64)
    {
        return error_response(StatusCode::PAYLOAD_TOO_LARGE, "Payload Too Large");
    }
    let method = request.method().clone();
    let uri = request.uri().clone();
    let headers = request.headers().clone();
    let request = match bounded_request(request, ACTIVITYPUB_INBOX_BODY_LIMIT_BYTES).await {
        Ok(request) => request,
        Err(response) => return response,
    };
    let Some(body) = request
        .extensions()
        .get::<BufferedRequestBody>()
        .map(|body| body.0.to_vec())
    else {
        return internal_error();
    };
    let verified = match verify_required_federation_signature(
        &state, client_ip, &method, &uri, &headers, &body,
    )
    .await
    {
        Ok(verified) => verified,
        Err(response) => return response,
    };
    let Some(actor_uri) = verified.actor_uri.as_deref() else {
        return signature_verification_failure();
    };
    let key_id = verified.key_id;
    let Ok(Some(remote_domain)) =
        remote_signature_domain(&key_id, &state.local_domain, &state.origin)
    else {
        return signature_verification_failure();
    };
    let activity = match serde_json::from_slice::<serde_json::Value>(&body) {
        Ok(serde_json::Value::Object(activity)) => serde_json::Value::Object(activity),
        Ok(_) | Err(_) => {
            return error_response(StatusCode::BAD_REQUEST, "Invalid ActivityPub JSON");
        }
    };
    let Ok(body) = String::from_utf8(body) else {
        return error_response(StatusCode::BAD_REQUEST, "Invalid ActivityPub JSON");
    };
    let logical_key = activitypub_inbox_logical_key(&activity, body.as_bytes(), actor_uri);
    let logical_key =
        activitypub_inbox_delivery_logical_key(&activity, &logical_key, target.account_id());
    let ordering_key = activitypub_inbox_ordering_key(actor_uri);
    let fingerprint: [u8; 32] = Sha256::digest(body.as_bytes()).into();
    let arguments = serde_json::json!({
        "body": body,
        "delivery_target_account_id": target.account_id(),
        "signature_key_id": key_id,
        "remote_domain": remote_domain,
    });
    let spec =
        JobSpec::new(Lane::Ingress, ACTIVITYPUB_INBOX_JOB_KIND, arguments).logical_key(logical_key);
    match queue
        .enqueue_ordered_once(&spec, &ordering_key, &fingerprint)
        .await
    {
        Ok(_) => {}
        Err(JobError::Conflict(message)) => {
            return error_response(StatusCode::CONFLICT, message);
        }
        Err(_) => return internal_error(),
    }
    Response::builder()
        .status(StatusCode::ACCEPTED)
        .body(Body::empty())
        .expect("static accepted response is valid")
}

pub(super) fn activitypub_inbox_logical_key(
    activity: &serde_json::Value,
    body: &[u8],
    verified_actor_uri: &str,
) -> String {
    let activity_id = activity
        .get("id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty());
    let source = activity_id.map_or_else(
        || format!("{}\n{verified_actor_uri}", String::from_utf8_lossy(body)),
        |activity_id| format!("{activity_id}\n{verified_actor_uri}"),
    );
    format!("activitypub:{:x}", Sha256::digest(source.as_bytes()))
}

pub(super) fn activitypub_inbox_delivery_logical_key(
    activity: &serde_json::Value,
    logical_key: &str,
    delivery_target_account_id: Option<i64>,
) -> String {
    let Some(delivery_target_account_id) = delivery_target_account_id else {
        return logical_key.to_owned();
    };
    let object = activity.get("object");
    let is_create = equals_or_includes(activity.get("type"), "Create");
    let is_write = ["Create", "Update", "Delete"]
        .into_iter()
        .any(|kind| equals_or_includes(activity.get("type"), kind));
    let is_note_write = is_create && object.is_some_and(serde_json::Value::is_string)
        || is_write
            && object
                .and_then(serde_json::Value::as_object)
                .is_some_and(|object| {
                    ["Note", "Question", "Tombstone"]
                        .into_iter()
                        .any(|kind| equals_or_includes(object.get("type"), kind))
                });
    if is_note_write {
        format!("{logical_key}:delivery:{delivery_target_account_id}")
    } else {
        logical_key.to_owned()
    }
}

pub(super) fn activitypub_inbox_ordering_key(actor_uri: &str) -> [u8; 32] {
    Sha256::digest(actor_uri.as_bytes()).into()
}

pub(super) fn remote_signature_domain(
    key_id: &str,
    local_domain: &str,
    local_origin: &Url,
) -> Result<Option<String>, ()> {
    if let Some(account) = key_id.strip_prefix("acct:") {
        let (_, domain) = account.rsplit_once('@').ok_or(())?;
        let domain = canonical_remote_domain(domain).map_err(|_| ())?;
        return Ok((!domain.eq_ignore_ascii_case(local_domain)).then_some(domain));
    }
    let url = Url::parse(key_id).map_err(|_| ())?;
    let authority = federation_url_authority(&url).ok_or(())?;
    let local_authority = federation_url_authority(local_origin).ok_or(())?;
    if authority.eq_ignore_ascii_case(&local_authority)
        || url
            .host_str()
            .is_some_and(|host| host.eq_ignore_ascii_case(local_domain))
    {
        return Ok(None);
    }
    // Key IDs commonly use fragments; only their validated origin is relevant here.
    let mut origin = url;
    origin.set_fragment(None);
    canonical_remote_domain_from_url(&origin)
        .map(Some)
        .map_err(|_| ())
}

pub(super) fn remote_signature_fetch_failure(error: &RemoteFetchError) -> Response<Body> {
    let status = if remote_signature_fetch_should_trip(error)
        || matches!(error, RemoteFetchError::DomainBudgetExceeded)
    {
        StatusCode::SERVICE_UNAVAILABLE
    } else {
        StatusCode::UNAUTHORIZED
    };
    error_response(status, "Request signature verification failed")
}

pub(super) fn remote_signature_fetch_should_trip(error: &RemoteFetchError) -> bool {
    matches!(
        error,
        RemoteFetchError::Client
            | RemoteFetchError::Dns
            | RemoteFetchError::NoAddresses
            | RemoteFetchError::Request
            | RemoteFetchError::BodyRead
    )
}

pub(super) fn federation_domain_matches(state: &WebState, domain: &str) -> bool {
    let origin_authority = federation_url_authority(&state.origin);
    state
        .allowed_hosts
        .iter()
        .filter(|allowed| {
            state.media_route_authority.as_deref() != Some(allowed.as_str())
                || origin_authority.as_deref() == Some(allowed.as_str())
        })
        .any(|allowed| allowed.eq_ignore_ascii_case(domain))
}

pub(super) fn federation_url_authority(url: &Url) -> Option<String> {
    let host = url.host_str()?;
    let default_port = match url.scheme() {
        "http" => Some(80),
        "https" => Some(443),
        _ => None,
    };
    Some(
        url.port()
            .filter(|port| Some(*port) != default_port)
            .map_or_else(|| host.to_owned(), |port| format!("{host}:{port}")),
    )
}

pub(super) async fn federation_local_account_id(
    state: &WebState,
    username: &str,
) -> Result<i64, Response<Body>> {
    match state
        .repository
        .rest_local_account_id_by_username(username)
        .await
    {
        Ok(Some(account_id)) => Ok(account_id),
        Ok(None) => Err(not_found()),
        Err(_) => Err(internal_error()),
    }
}

#[allow(clippy::too_many_lines)]
pub(super) async fn federation_webfinger(
    State(state): State<WebState>,
    Query(parameters): Query<HashMap<String, String>>,
) -> Response<Body> {
    let Some(resource) = parameters.get("resource") else {
        return error_response(StatusCode::BAD_REQUEST, "Missing resource");
    };
    let is_http_resource = resource
        .get(..7)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("http://"))
        || resource
            .get(..8)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("https://"));
    let account_id = if is_http_resource {
        let Ok(url) = Url::parse(resource) else {
            return not_found();
        };
        let Some(authority) = federation_url_authority(&url) else {
            return not_found();
        };
        if !federation_domain_matches(&state, &authority) {
            return not_found();
        }
        let path = url.path().trim_end_matches('/');
        if path == "/actor" {
            -99
        } else if let Some(username) = path.strip_prefix("/@") {
            match state
                .repository
                .rest_local_account_id_by_username(username)
                .await
            {
                Ok(Some(account_id)) => account_id,
                Ok(None) => return not_found(),
                Err(_) => return internal_error(),
            }
        } else if let Some(username) = path.strip_prefix("/users/") {
            if username.is_empty() || username.contains('/') {
                return not_found();
            }
            match state
                .repository
                .rest_local_account_id_by_username(username)
                .await
            {
                Ok(Some(account_id)) => account_id,
                Ok(None) => return not_found(),
                Err(_) => return internal_error(),
            }
        } else if let Some(id) = path
            .strip_prefix("/ap/users/")
            .and_then(activitypub_path_id)
        {
            id
        } else {
            return not_found();
        }
    } else {
        let resource = resource.strip_prefix("acct:").unwrap_or(resource);
        let mut parts = resource.split('@');
        let Some(username) = parts.next() else {
            return error_response(StatusCode::BAD_REQUEST, "Invalid resource");
        };
        let Some(domain) = parts.next() else {
            return error_response(StatusCode::BAD_REQUEST, "Invalid resource");
        };
        if username.is_empty()
            || domain.is_empty()
            || parts.next().is_some()
            || !federation_domain_matches(&state, domain)
        {
            return not_found();
        }
        if username.eq_ignore_ascii_case(&state.local_domain)
            || username.eq_ignore_ascii_case(state.origin.host_str().unwrap_or_default())
        {
            -99
        } else {
            match state
                .repository
                .rest_local_account_id_by_username(username)
                .await
            {
                Ok(Some(account_id)) => account_id,
                Ok(None) => return not_found(),
                Err(_) => return internal_error(),
            }
        }
    };
    let account = match state.repository.account(account_id).await {
        Ok(Some(account)) if account.id == -99 || account.domain.is_none() => account,
        Ok(Some(_) | None) => return not_found(),
        Err(_) => return internal_error(),
    };
    if account.suspended_at.is_some() {
        let Ok(temporary) = state
            .repository
            .account_has_deletion_request(account.id)
            .await
        else {
            return internal_error();
        };
        if !temporary {
            return raw_response(StatusCode::GONE, "text/plain; charset=utf-8", Vec::new());
        }
    }
    activity_response(
        StatusCode::OK,
        JRD_JSON,
        activitypub::webfinger(
            &state.origin,
            &state.local_domain,
            &state.media_root_url,
            state.instance_runtime.limited_federation,
            &account,
        ),
    )
}

pub(super) async fn federation_host_meta(
    State(state): State<WebState>,
    headers: HeaderMap,
    uri: Uri,
) -> Response<Body> {
    #[allow(clippy::case_sensitive_file_extension_comparisons)]
    let accept_json = uri.path().ends_with(".json")
        || headers
            .get("accept")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.contains("application/json"));
    let (content_type, body) = activitypub::host_meta(&state.origin, accept_json);
    raw_response(StatusCode::OK, &content_type, body)
}

pub(super) async fn federation_nodeinfo_discovery(State(state): State<WebState>) -> Response<Body> {
    activity_response(
        StatusCode::OK,
        "application/json; charset=utf-8",
        activitypub::nodeinfo_discovery(&state.origin),
    )
}

pub(super) async fn federation_nodeinfo(State(state): State<WebState>) -> Response<Body> {
    let Ok(instance) = state.instance().await else {
        return internal_error();
    };
    activity_response(
        StatusCode::OK,
        "application/json; charset=utf-8",
        activitypub::nodeinfo(
            &instance.runtime.version,
            &instance.title,
            &instance.short_description,
            instance.user_count,
            instance.status_count,
            instance.activity.active_month,
            instance.activity.active_halfyear,
            instance.registrations_mode != "none" && !instance.runtime.single_user_mode,
        ),
    )
}

pub(super) async fn federation_actor_instance(
    State(state): State<WebState>,
    headers: HeaderMap,
    uri: Uri,
) -> Response<Body> {
    federation_actor_response(&state, -99, None, &headers, &uri).await
}

pub(super) async fn federation_emoji(
    State(state): State<WebState>,
    Path(id): Path<String>,
) -> Response<Body> {
    let Some(id) = activitypub_path_id(&id) else {
        return not_found();
    };
    match state.repository.activitypub_emoji(id).await {
        Ok(Some(emoji)) => activity_response(
            StatusCode::OK,
            ACTIVITY_JSON,
            activitypub::emoji(&state.origin, &state.media_root_url, &emoji),
        ),
        Ok(None) => not_found(),
        Err(_) => internal_error(),
    }
}

pub(super) async fn federation_actor_username(
    State(state): State<WebState>,
    Extension(metadata): Extension<RequestMetadata>,
    Path(username): Path<String>,
    headers: HeaderMap,
    uri: Uri,
) -> Response<Body> {
    if !accepts_activitypub(&headers) {
        if uri.path().starts_with("/@") {
            return frontend_html_response(&state, uri.path(), &headers).await;
        }
        return html_redirect(&state.origin, &format!("/@{username}"));
    }
    let account_id = match federation_local_account_id(&state, &username).await {
        Ok(account_id) => account_id,
        Err(response) => return response,
    };
    federation_actor_response(&state, account_id, Some(metadata.client_ip), &headers, &uri).await
}

pub(super) async fn federation_actor_id(
    State(state): State<WebState>,
    Extension(metadata): Extension<RequestMetadata>,
    Path(id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
) -> Response<Body> {
    let Some(id) = activitypub_path_id(&id) else {
        return not_found();
    };
    federation_actor_response(&state, id, Some(metadata.client_ip), &headers, &uri).await
}

pub(super) async fn federation_actor_response(
    state: &WebState,
    account_id: i64,
    client_ip: Option<IpAddr>,
    headers: &HeaderMap,
    uri: &Uri,
) -> Response<Body> {
    if !accepts_activitypub(headers) {
        return error_response(
            StatusCode::NOT_ACCEPTABLE,
            "ActivityPub representation required",
        );
    }
    if let Some(client_ip) = client_ip
        && let Err(response) =
            verify_optional_federation_signature(state, client_ip, uri, headers).await
    {
        return response;
    }
    let account = match federation_account(state, account_id, false).await {
        Ok(account) => account,
        Err(response) => return response,
    };
    if account.suspended_at.is_some() {
        let Ok(temporary) = state
            .repository
            .account_has_deletion_request(account.id)
            .await
        else {
            return internal_error();
        };
        if !temporary {
            return error_response(StatusCode::GONE, "Unavailable account");
        }
    }
    let Ok(hashtags) = state
        .repository
        .activitypub_account_hashtags(account.id)
        .await
    else {
        return internal_error();
    };
    let Ok(emojis) = state
        .repository
        .activitypub_account_emojis(account.id)
        .await
    else {
        return internal_error();
    };
    activity_response(
        StatusCode::OK,
        ACTIVITY_JSON,
        activitypub::actor_with_media(
            &state.origin,
            &state.local_domain,
            &state.media_root_url,
            &account,
            &hashtags,
            &emojis,
        ),
    )
}

pub(super) async fn federation_quote_authorization_username(
    State(state): State<WebState>,
    Extension(metadata): Extension<RequestMetadata>,
    Path((username, quote_id)): Path<(String, String)>,
    headers: HeaderMap,
    uri: Uri,
) -> Response<Body> {
    let account_id = match federation_local_account_id(&state, &username).await {
        Ok(account_id) => account_id,
        Err(response) => return response,
    };
    federation_quote_authorization_response(
        &state,
        account_id,
        metadata.client_ip,
        &quote_id,
        &headers,
        &uri,
    )
    .await
}

pub(super) async fn federation_quote_authorization_id(
    State(state): State<WebState>,
    Extension(metadata): Extension<RequestMetadata>,
    Path((account_id, quote_id)): Path<(String, String)>,
    headers: HeaderMap,
    uri: Uri,
) -> Response<Body> {
    let Some(account_id) = activitypub_path_id(&account_id) else {
        return not_found();
    };
    federation_quote_authorization_response(
        &state,
        account_id,
        metadata.client_ip,
        &quote_id,
        &headers,
        &uri,
    )
    .await
}

pub(super) async fn federation_quote_authorization_response(
    state: &WebState,
    quoted_account_id: i64,
    client_ip: IpAddr,
    quote_id: &str,
    headers: &HeaderMap,
    uri: &Uri,
) -> Response<Body> {
    if !accepts_activitypub(headers) {
        return error_response(
            StatusCode::NOT_ACCEPTABLE,
            "ActivityPub representation required",
        );
    }
    let Some(quote_id) = route_path_id(quote_id) else {
        return not_found();
    };
    let quoted_account = match federation_account(state, quoted_account_id, true).await {
        Ok(account) => account,
        Err(response) => return response,
    };
    let viewer_account_id = match federation_status_viewer(state, client_ip, uri, headers).await {
        Ok(viewer_account_id) => viewer_account_id,
        Err(response) => return response,
    };
    let Some(quote) = (match state
        .repository
        .activitypub_quote_authorization(quoted_account.id, quote_id)
        .await
    {
        Ok(quote) => quote,
        Err(_) => return internal_error(),
    }) else {
        return not_found();
    };
    let Some(quoted_status_id) = quote.quoted_status_id else {
        return not_found();
    };
    let Some(quoted_status) = (match state.repository.status(quoted_status_id).await {
        Ok(status) => status,
        Err(_) => return internal_error(),
    }) else {
        return not_found();
    };
    let authorized = match state
        .repository
        .rest_authorized_status_ids(&[quoted_status.id], viewer_account_id)
        .await
    {
        Ok(ids) => ids == [quoted_status.id],
        Err(_) => return internal_error(),
    };
    if !authorized {
        return not_found();
    }
    let Some(quoting_status) = (match state.repository.status(quote.status_id).await {
        Ok(status) => status,
        Err(_) => return internal_error(),
    }) else {
        return not_found();
    };
    let Some(quoting_account) = (match state.repository.account(quote.account_id).await {
        Ok(account) => account,
        Err(_) => return internal_error(),
    }) else {
        return not_found();
    };
    signature_sensitive_activity_response(
        headers,
        activitypub::quote_authorization(
            &state.origin,
            &quoted_account,
            &quoted_status,
            &quoting_account,
            &quoting_status,
            quote.id,
        ),
    )
}

pub(super) async fn federation_note_username(
    State(state): State<WebState>,
    Extension(metadata): Extension<RequestMetadata>,
    Path((username, id)): Path<(String, String)>,
    headers: HeaderMap,
    uri: Uri,
) -> Response<Body> {
    if !accepts_activitypub(&headers) {
        return html_redirect(&state.origin, &format!("/@{username}/{id}"));
    }
    let response = match federation_local_account_id(&state, &username).await {
        Ok(account_id) => {
            federation_note_response(&state, account_id, metadata.client_ip, &id, &headers, &uri)
                .await
        }
        Err(response) => response,
    };
    finalize_activitypub_status_response(state.instance_runtime.limited_federation, response)
}

pub(super) async fn federation_note_id(
    State(state): State<WebState>,
    Extension(metadata): Extension<RequestMetadata>,
    Path((account_id, id)): Path<(String, String)>,
    headers: HeaderMap,
    uri: Uri,
) -> Response<Body> {
    let response = match activitypub_path_id(&account_id) {
        Some(account_id) => {
            federation_note_response(&state, account_id, metadata.client_ip, &id, &headers, &uri)
                .await
        }
        None => not_found(),
    };
    finalize_activitypub_status_response(state.instance_runtime.limited_federation, response)
}

pub(super) async fn federation_status_collection_username(
    State(state): State<WebState>,
    Extension(metadata): Extension<RequestMetadata>,
    Path((username, status_id, collection)): Path<(String, String, String)>,
    Query(parameters): Query<HashMap<String, String>>,
    headers: HeaderMap,
    uri: Uri,
) -> Response<Body> {
    if !matches!(collection.as_str(), "replies" | "likes" | "shares") {
        return not_found();
    }
    if !accepts_activitypub(&headers) {
        return html_redirect(
            &state.origin,
            &format!("/@{username}/{status_id}/{collection}"),
        );
    }
    let account_id = match federation_local_account_id(&state, &username).await {
        Ok(account_id) => account_id,
        Err(response) => return response,
    };
    federation_status_collection_response(
        &state,
        account_id,
        &status_id,
        &collection,
        metadata.client_ip,
        parameters,
        &headers,
        &uri,
    )
    .await
}

pub(super) async fn federation_status_collection_id(
    State(state): State<WebState>,
    Extension(metadata): Extension<RequestMetadata>,
    Path((account_id, status_id, collection)): Path<(String, String, String)>,
    Query(parameters): Query<HashMap<String, String>>,
    headers: HeaderMap,
    uri: Uri,
) -> Response<Body> {
    if !matches!(collection.as_str(), "replies" | "likes" | "shares") {
        return not_found();
    }
    let Some(account_id) = activitypub_path_id(&account_id) else {
        return not_found();
    };
    federation_status_collection_response(
        &state,
        account_id,
        &status_id,
        &collection,
        metadata.client_ip,
        parameters,
        &headers,
        &uri,
    )
    .await
}

#[allow(
    clippy::manual_let_else,
    clippy::too_many_arguments,
    clippy::too_many_lines
)]
pub(super) async fn federation_status_collection_response(
    state: &WebState,
    account_id: i64,
    status_id: &str,
    collection: &str,
    client_ip: IpAddr,
    parameters: HashMap<String, String>,
    headers: &HeaderMap,
    uri: &Uri,
) -> Response<Body> {
    if !accepts_activitypub(headers) {
        return error_response(
            StatusCode::NOT_ACCEPTABLE,
            "ActivityPub representation required",
        );
    }
    let Some(status_id) = route_path_id(status_id) else {
        return not_found();
    };
    let account = match federation_account(state, account_id, true).await {
        Ok(account) => account,
        Err(response) => return response,
    };
    let viewer_account_id = match federation_signed_viewer(state, client_ip, uri, headers).await {
        Ok(viewer_account_id) => viewer_account_id,
        Err(response) => return response,
    };
    let authorized = match state
        .repository
        .rest_authorized_status_ids(&[status_id], viewer_account_id)
        .await
    {
        Ok(ids) => ids == [status_id],
        Err(_) => return internal_error(),
    };
    if !authorized {
        return not_found();
    }
    let status = match state.repository.status(status_id).await {
        Ok(Some(status)) if status.account_id == account.id => status,
        Ok(_) => return not_found(),
        Err(_) => return internal_error(),
    };
    match collection {
        "replies" => {
            let only_other_accounts = parameters
                .get("only_other_accounts")
                .is_some_and(|value| activitypub_truthy(value));
            let min_id = parameters
                .get("min_id")
                .and_then(|value| route_path_id(value));
            let Ok(replies) = state
                .repository
                .activitypub_reply_statuses(account.id, status.id, only_other_accounts, min_id, 60)
                .await
            else {
                return internal_error();
            };
            let Ok(items) = activitypub_reply_items(state, &replies).await else {
                return internal_error();
            };
            let page_requested = parameters
                .get("page")
                .is_some_and(|value| activitypub_truthy(value));
            let base = activitypub::replies_url(&state.origin, &account, &status);
            let next = if only_other_accounts {
                (replies.len() == 60).then(|| {
                    activitypub_replies_next_url(&base, replies.last().map(|reply| reply.id), true)
                })
            } else {
                let next_only_other_accounts = replies
                    .last()
                    .is_none_or(|reply| reply.account_id != account.id)
                    || replies.len() < 60;
                Some(activitypub_replies_next_url(
                    &base,
                    (!next_only_other_accounts)
                        .then(|| replies.last().map(|reply| reply.id))
                        .flatten(),
                    next_only_other_accounts,
                ))
            };
            let page_id = activitypub_replies_page_id(&base, &parameters);
            let mut page = serde_json::json!({
                "@context": activitypub::ACTIVITY_STREAMS_CONTEXT,
                "id": page_id,
                "type": "CollectionPage",
                "partOf": base.clone(),
                "items": items
            });
            if let Some(next) = next {
                page["next"] = serde_json::Value::String(next);
            }
            let value = if page_requested {
                page
            } else {
                serde_json::json!({
                    "@context": activitypub::ACTIVITY_STREAMS_CONTEXT,
                    "id": base,
                    "type": "Collection",
                    "first": page
                })
            };
            signature_sensitive_activity_response(headers, value)
        }
        "likes" | "shares" => {
            let (base, count) = match activitypub_status_counts(state, status.id).await {
                Ok((favourites_count, _reblogs_count)) if collection == "likes" => (
                    activitypub::likes_url(&state.origin, &account, &status),
                    favourites_count,
                ),
                Ok((_favourites_count, reblogs_count)) => (
                    activitypub::shares_url(&state.origin, &account, &status),
                    reblogs_count,
                ),
                Err(()) => return internal_error(),
            };
            signature_sensitive_activity_response(
                headers,
                serde_json::json!({
                    "@context": activitypub::ACTIVITY_STREAMS_CONTEXT,
                    "id": base,
                    "type": "Collection",
                    "totalItems": count
                }),
            )
        }
        _ => not_found(),
    }
}

pub(super) async fn activitypub_reply_items(
    state: &WebState,
    replies: &[crate::mastodon::Status],
) -> Result<Vec<serde_json::Value>, ()> {
    let mut items = Vec::with_capacity(replies.len());
    for reply in replies {
        if reply.local == Some(true) || reply.uri.is_none() {
            let Some(reply_account) = state
                .repository
                .account(reply.account_id)
                .await
                .map_err(|_| ())?
            else {
                return Err(());
            };
            items.push(federation_note_value(state, reply, &reply_account).await?);
        } else {
            items.push(
                reply
                    .uri
                    .clone()
                    .map_or(serde_json::Value::Null, serde_json::Value::String),
            );
        }
    }
    Ok(items)
}

pub(super) fn activitypub_replies_page_id(
    base: &str,
    parameters: &HashMap<String, String>,
) -> String {
    let mut query = Vec::new();
    if let Some(value) = parameters.get("only_other_accounts") {
        query.push(format!("only_other_accounts={value}"));
    }
    if let Some(value) = parameters.get("min_id") {
        query.push(format!("min_id={value}"));
    }
    query.push("page=true".to_owned());
    format!("{base}?{}", query.join("&"))
}

pub(super) fn activitypub_replies_next_url(
    base: &str,
    min_id: Option<i64>,
    only_other_accounts: bool,
) -> String {
    let mut query = Vec::new();
    if only_other_accounts {
        query.push("only_other_accounts=true".to_owned());
    }
    if let Some(min_id) = min_id {
        query.push(format!("min_id={min_id}"));
    }
    query.push("page=true".to_owned());
    format!("{base}?{}", query.join("&"))
}

pub(super) async fn federation_status_activity_username(
    State(state): State<WebState>,
    Extension(metadata): Extension<RequestMetadata>,
    Path((username, id)): Path<(String, String)>,
    headers: HeaderMap,
    uri: Uri,
) -> Response<Body> {
    if !accepts_activitypub(&headers) {
        return html_redirect(&state.origin, &format!("/@{username}/{id}"));
    }
    let response = match federation_local_account_id(&state, &username).await {
        Ok(account_id) => {
            federation_status_activity_response(
                &state,
                account_id,
                metadata.client_ip,
                &id,
                &headers,
                &uri,
            )
            .await
        }
        Err(response) => response,
    };
    finalize_activitypub_status_response(state.instance_runtime.limited_federation, response)
}

pub(super) async fn federation_status_activity_id(
    State(state): State<WebState>,
    Extension(metadata): Extension<RequestMetadata>,
    Path((account_id, id)): Path<(String, String)>,
    headers: HeaderMap,
    uri: Uri,
) -> Response<Body> {
    let response = match activitypub_path_id(&account_id) {
        Some(account_id) => {
            federation_status_activity_response(
                &state,
                account_id,
                metadata.client_ip,
                &id,
                &headers,
                &uri,
            )
            .await
        }
        None => not_found(),
    };
    finalize_activitypub_status_response(state.instance_runtime.limited_federation, response)
}

#[allow(clippy::manual_let_else)]
pub(super) async fn federation_note_response(
    state: &WebState,
    account_id: i64,
    client_ip: IpAddr,
    id: &str,
    headers: &HeaderMap,
    uri: &Uri,
) -> Response<Body> {
    if !accepts_activitypub(headers) {
        return error_response(
            StatusCode::NOT_ACCEPTABLE,
            "ActivityPub representation required",
        );
    }
    let Some(status_id) = route_path_id(id) else {
        return not_found();
    };
    let account = match federation_account(state, account_id, true).await {
        Ok(account) => account,
        Err(response) => return response,
    };
    let viewer_account_id = match federation_status_viewer(state, client_ip, uri, headers).await {
        Ok(viewer_account_id) => viewer_account_id,
        Err(response) => return response,
    };
    let authorized = match state
        .repository
        .rest_authorized_status_ids(&[status_id], viewer_account_id)
        .await
    {
        Ok(ids) => ids == [status_id],
        Err(_) => return internal_error(),
    };
    if !authorized {
        return not_found();
    }
    let Some(status) = (match state.repository.status(status_id).await {
        Ok(status) => status,
        Err(_) => return internal_error(),
    }) else {
        return not_found();
    };
    if status.account_id != account_id {
        return not_found();
    }
    if let Some(source_id) = status.reblog_of_id {
        let Some(source) = (match state.repository.status(source_id).await {
            Ok(source) => source,
            Err(_) => return internal_error(),
        }) else {
            return not_found();
        };
        let Some(source_account) = (match state.repository.account(source.account_id).await {
            Ok(source_account) => source_account,
            Err(_) => return internal_error(),
        }) else {
            return internal_error();
        };
        return activitypub_status_redirect(&activitypub::status_url(
            &state.origin,
            &source_account,
            &source,
        ));
    }
    let pending_quote = match state
        .repository
        .activitypub_status_has_pending_quote(status.id)
        .await
    {
        Ok(pending_quote) => pending_quote,
        Err(_) => return internal_error(),
    };
    let Ok(note) = federation_note_value(state, &status, &account).await else {
        return internal_error();
    };
    let Some(note_id) = note["id"].as_str().map(str::to_owned) else {
        return internal_error();
    };
    signature_sensitive_status_response(
        headers,
        note,
        &note_id,
        !state.instance_runtime.limited_federation,
        matches!(
            status.visibility,
            crate::mastodon::StatusVisibility::Public | crate::mastodon::StatusVisibility::Unlisted
        ),
        ActivityPubStatusDocument::Note { pending_quote },
    )
}

#[allow(clippy::too_many_lines)]
pub(super) async fn federation_status_activity_response(
    state: &WebState,
    account_id: i64,
    client_ip: IpAddr,
    id: &str,
    headers: &HeaderMap,
    uri: &Uri,
) -> Response<Body> {
    if !accepts_activitypub(headers) {
        return error_response(
            StatusCode::NOT_ACCEPTABLE,
            "ActivityPub representation required",
        );
    }
    let account = match federation_account(state, account_id, true).await {
        Ok(account) => account,
        Err(response) => return response,
    };
    let Some(status_id) = route_path_id(id) else {
        return not_found();
    };
    let viewer_account_id = match federation_status_viewer(state, client_ip, uri, headers).await {
        Ok(viewer_account_id) => viewer_account_id,
        Err(response) => return response,
    };
    let authorized = match state
        .repository
        .rest_authorized_status_ids(&[status_id], viewer_account_id)
        .await
    {
        Ok(ids) => ids == [status_id],
        Err(_) => return internal_error(),
    };
    if !authorized {
        return not_found();
    }
    let Some(status) = (match state.repository.status(status_id).await {
        Ok(status) => status,
        Err(_) => return internal_error(),
    }) else {
        return not_found();
    };
    if status.account_id != account.id {
        return not_found();
    }
    let Ok(note) = federation_note_value(state, &status, &account).await else {
        return internal_error();
    };
    let activity = if let Some(source_id) = status.reblog_of_id {
        let Some(source) = (match state.repository.status(source_id).await {
            Ok(source) => source,
            Err(_) => return internal_error(),
        }) else {
            return not_found();
        };
        let Some(source_account) = (match state.repository.account(source.account_id).await {
            Ok(source_account) => source_account,
            Err(_) => return internal_error(),
        }) else {
            return internal_error();
        };
        let object = if status.visibility == crate::mastodon::StatusVisibility::Private
            && source.local == Some(true)
            && source_account.id == account.id
        {
            match federation_note_value(state, &source, &source_account).await {
                Ok(note) => note,
                Err(()) => return internal_error(),
            }
        } else {
            serde_json::Value::String(activitypub::status_uri(
                &state.origin,
                &source_account,
                &source,
            ))
        };
        let mut cc = note["cc"].clone();
        if let serde_json::Value::Array(values) = &mut cc {
            values.push(serde_json::Value::String(activitypub::actor_url(
                &state.origin,
                &source_account,
            )));
        }
        activitypub::status_activity(
            &state.origin,
            &account,
            &status,
            object,
            note["to"].clone(),
            cc,
        )
    } else {
        activitypub::status_activity(
            &state.origin,
            &account,
            &status,
            note,
            serde_json::Value::Null,
            serde_json::Value::Null,
        )
    };
    let activity_uri = activitypub::status_uri(&state.origin, &account, &status);
    signature_sensitive_status_response(
        headers,
        activity,
        &activity_uri,
        !state.instance_runtime.limited_federation,
        matches!(
            status.visibility,
            crate::mastodon::StatusVisibility::Public | crate::mastodon::StatusVisibility::Unlisted
        ),
        ActivityPubStatusDocument::Activity,
    )
}

pub(super) fn signature_sensitive_activity_response(
    headers: &HeaderMap,
    value: serde_json::Value,
) -> Response<Body> {
    let mut response = activity_response(StatusCode::OK, ACTIVITY_JSON, value);
    response
        .headers_mut()
        .insert(VARY, HeaderValue::from_static("Accept, Signature"));
    if headers.contains_key("signature") {
        response
            .headers_mut()
            .insert(CACHE_CONTROL, HeaderValue::from_static(PRIVATE_CACHE));
    }
    response
}

pub(super) fn signature_sensitive_status_response(
    headers: &HeaderMap,
    value: serde_json::Value,
    activity_uri: &str,
    public_fetch_mode: bool,
    distributable: bool,
    document: ActivityPubStatusDocument,
) -> Response<Body> {
    let mut response = activity_response(StatusCode::OK, ACTIVITY_JSON, value);
    let vary = if public_fetch_mode {
        ACTIVITYPUB_STATUS_PUBLIC_VARY
    } else {
        ACTIVITYPUB_STATUS_AUTHORIZED_VARY
    };
    response
        .headers_mut()
        .insert(VARY, HeaderValue::from_static(vary));
    let has_viewer_credentials = [COOKIE.as_str(), AUTHORIZATION.as_str(), "signature"]
        .into_iter()
        .any(|name| request_header_is_nonempty(headers, name));
    let cache_control = if public_fetch_mode && distributable && !has_viewer_credentials {
        match document {
            ActivityPubStatusDocument::Note {
                pending_quote: true,
            } => ACTIVITYPUB_STATUS_PENDING_QUOTE_CACHE,
            ActivityPubStatusDocument::Note {
                pending_quote: false,
            }
            | ActivityPubStatusDocument::Activity => ACTIVITYPUB_STATUS_PUBLIC_CACHE,
        }
    } else if public_fetch_mode
        && !distributable
        && matches!(document, ActivityPubStatusDocument::Activity)
    {
        ACTIVITYPUB_STATUS_PRIVATE_ACTIVITY_CACHE
    } else {
        PRIVATE_CACHE
    };
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static(cache_control));
    response.headers_mut().insert(
        "link",
        HeaderValue::from_str(&format!(
            "<{activity_uri}>; rel=\"alternate\"; type=\"application/activity+json\""
        ))
        .expect("ActivityPub status link header is valid"),
    );
    response
}

pub(super) fn activitypub_status_redirect(location: &str) -> Response<Body> {
    Response::builder()
        .status(StatusCode::FOUND)
        .header(LOCATION, location)
        .body(Body::empty())
        .expect("ActivityPub status redirect headers are valid")
}

pub(super) fn finalize_activitypub_status_response(
    limited_federation: bool,
    mut response: Response<Body>,
) -> Response<Body> {
    if response.status().is_success() {
        return response;
    }
    let vary = if limited_federation {
        ACTIVITYPUB_STATUS_AUTHORIZED_VARY
    } else {
        ACTIVITYPUB_STATUS_PUBLIC_VARY
    };
    response
        .headers_mut()
        .insert(VARY, HeaderValue::from_static(vary));
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static(PRIVATE_CACHE));
    response
}

pub(super) async fn federation_status_viewer(
    state: &WebState,
    client_ip: IpAddr,
    uri: &Uri,
    headers: &HeaderMap,
) -> Result<Option<i64>, Response<Body>> {
    let browser_viewer = optional_browser_session_viewer(state, headers).await?;
    let signature_key_id = match verify_federation_request(
        state,
        client_ip,
        &Method::GET,
        uri,
        headers,
        &[],
        false,
        true,
    )
    .await
    {
        Ok(signature_key_id) => signature_key_id,
        Err(response)
            if !state.instance_runtime.limited_federation
                && (response.status().is_client_error()
                    || response.status() == StatusCode::SERVICE_UNAVAILABLE) =>
        {
            return Ok(browser_viewer.or(optional_viewer(state, headers, READ_STATUSES).await?));
        }
        Err(response) => return Err(response),
    };
    if browser_viewer.is_some() {
        return Ok(browser_viewer);
    }
    if let Some(signature) = signature_key_id {
        return state
            .repository
            .activitypub_signature_key(&signature.key_id, state.origin.as_str())
            .await
            .map(|key| key.map(|key| key.account_id))
            .map_err(|_| internal_error());
    }
    optional_viewer(state, headers, READ_STATUSES).await
}

pub(super) async fn federation_signed_viewer(
    state: &WebState,
    client_ip: IpAddr,
    uri: &Uri,
    headers: &HeaderMap,
) -> Result<Option<i64>, Response<Body>> {
    let Some(signature) = verify_federation_request(
        state,
        client_ip,
        &Method::GET,
        uri,
        headers,
        &[],
        false,
        true,
    )
    .await?
    else {
        return Ok(None);
    };
    state
        .repository
        .activitypub_signature_key(&signature.key_id, state.origin.as_str())
        .await
        .map(|key| key.map(|key| key.account_id))
        .map_err(|_| internal_error())
}

pub(super) async fn optional_browser_session_viewer(
    state: &WebState,
    headers: &HeaderMap,
) -> Result<Option<i64>, Response<Body>> {
    let Some(session_id) = request_cookie(headers, BROWSER_SESSION_COOKIE) else {
        return Ok(None);
    };
    match state.repository.browser_session(session_id).await {
        Ok(Some(session)) => Ok(Some(session.account_id)),
        Ok(None) => Ok(None),
        Err(_) => Err(internal_error()),
    }
}

pub(super) async fn federation_note_value(
    state: &WebState,
    status: &crate::mastodon::Status,
    account: &Account,
) -> Result<serde_json::Value, ()> {
    let Ok(media) = state.repository.media_attachments(status.id).await else {
        return Err(());
    };
    let Ok(mention_rows) = state.repository.mentions(status.id).await else {
        return Err(());
    };
    let mut mentions = Vec::new();
    for mention in mention_rows {
        if !mention.silent
            && let Ok(Some(target)) = state.repository.account(mention.account_id).await
        {
            mentions.push((mention, target));
        }
    }
    let hashtags = match state.repository.rest_status_tag_rows(&[status.id]).await {
        Ok(rows) => rows
            .into_iter()
            .map(|row| (row.name.clone(), row.display_name.unwrap_or(row.name)))
            .collect::<Vec<_>>(),
        Err(_) => return Err(()),
    };
    let (favourites_count, reblogs_count) = activitypub_status_counts(state, status.id).await?;
    let emojis = state
        .repository
        .activitypub_status_emojis(status.id)
        .await
        .map_err(|_| ())?;
    let Ok(quoted_link) = activitypub_quote_url(state, status.id).await else {
        return Err(());
    };
    let Ok(quoted_identifier) = activitypub_quote_uri(state, status.id).await else {
        return Err(());
    };
    let Ok(quote_authorization) = activitypub_quote_authorization(state, status.id).await else {
        return Err(());
    };
    let Ok(replies) = activitypub_replies(state, account, status).await else {
        return Err(());
    };
    let Ok((in_reply_to_url, in_reply_to_atom_uri, conversation)) =
        activitypub_note_metadata(state, status).await
    else {
        return Err(());
    };
    let mut object = activitypub::note(
        &state.origin,
        &state.local_domain,
        status,
        account,
        &state.media_root_url,
        &media,
        &mentions,
        &hashtags,
        &emojis,
        quoted_link.as_deref(),
        in_reply_to_url.as_deref(),
        in_reply_to_atom_uri.as_deref(),
        conversation.as_deref(),
        quoted_identifier.as_deref(),
        quote_authorization.as_deref(),
        replies,
        favourites_count,
        reblogs_count,
    );
    if let Some(poll_id) = status.poll_id {
        let poll = state
            .repository
            .poll(poll_id)
            .await
            .map_err(|_| ())?
            .ok_or(())?;
        object = activitypub::question(object, &poll, Utc::now().naive_utc());
    }
    Ok(object)
}

pub(super) async fn activitypub_status_counts(
    state: &WebState,
    status_id: i64,
) -> Result<(i64, i64), ()> {
    let status_stats = state
        .repository
        .status_stat(status_id)
        .await
        .map_err(|_| ())?;
    Ok(status_stats.map_or((0, 0), |status_stats| {
        (
            status_stats.favourites_count.max(0),
            status_stats.reblogs_count.max(0),
        )
    }))
}

pub(super) async fn activitypub_quote_url(
    state: &WebState,
    status_id: i64,
) -> Result<Option<String>, ()> {
    let target = state
        .repository
        .activitypub_quote_target(status_id)
        .await
        .map_err(|_| ())?;
    Ok(target.map(|target| {
        if target.local {
            state
                .origin
                .join(&format!("@{}/{}", target.username, target.id))
                .expect("origin is absolute")
                .to_string()
        } else {
            target.url.unwrap_or_default()
        }
    }))
}

pub(super) async fn activitypub_quote_uri(
    state: &WebState,
    status_id: i64,
) -> Result<Option<String>, ()> {
    let target = state
        .repository
        .activitypub_quote_target(status_id)
        .await
        .map_err(|_| ())?;
    Ok(target.map(|target| {
        if let Some(uri) = target.uri {
            uri
        } else if target.local {
            activitypub::local_status_uri(
                &state.origin,
                target.account_id,
                &target.username,
                target.id_scheme,
                target.id,
            )
        } else {
            target.url.unwrap_or_default()
        }
    }))
}

pub(super) async fn activitypub_quote_authorization(
    state: &WebState,
    status_id: i64,
) -> Result<Option<String>, ()> {
    let target = state
        .repository
        .activitypub_quote_target(status_id)
        .await
        .map_err(|_| ())?;
    Ok(target
        .map(|target| {
            if target.quoted_account_local {
                activitypub::local_quote_authorization_url(
                    &state.origin,
                    target.account_id,
                    &target.username,
                    target.id_scheme,
                    target.quote_id,
                )
            } else {
                target
                    .approval_uri
                    .filter(|uri| !crate::paperclip::rails_blank(uri))
                    .unwrap_or_default()
            }
        })
        .filter(|uri| !uri.is_empty()))
}

pub(super) async fn activitypub_note_metadata(
    state: &WebState,
    status: &crate::mastodon::Status,
) -> Result<(Option<String>, Option<String>, Option<String>), ()> {
    let (in_reply_to_url, in_reply_to_atom_uri) =
        match (status.in_reply_to_id, status.in_reply_to_account_id) {
            (Some(parent_id), Some(parent_account_id)) => {
                let parent = state.repository.status(parent_id).await.map_err(|_| ())?;
                let account = state
                    .repository
                    .account(parent_account_id)
                    .await
                    .map_err(|_| ())?;
                match parent.zip(account) {
                    Some((parent, account)) => {
                        let atom_uri = if account.domain.is_none() {
                            Some(parent.uri.clone().unwrap_or_else(|| {
                                format!(
                                    "tag:{},{}:objectId={}:objectType=Status",
                                    state.local_domain,
                                    parent.created_at.date(),
                                    parent.id
                                )
                            }))
                        } else {
                            parent.uri.clone()
                        };
                        (
                            Some(activitypub::status_uri(&state.origin, &account, &parent)),
                            atom_uri,
                        )
                    }
                    None => (None, None),
                }
            }
            _ => (None, None),
        };
    let conversation = match status.conversation_id {
        Some(conversation_id) => state
            .repository
            .conversation(conversation_id)
            .await
            .map_err(|_| ())?
            .and_then(|conversation| conversation.uri),
        None => None,
    };
    Ok((in_reply_to_url, in_reply_to_atom_uri, conversation))
}

pub(super) async fn activitypub_replies(
    state: &WebState,
    account: &Account,
    status: &crate::mastodon::Status,
) -> Result<Option<serde_json::Value>, ()> {
    if account.domain.is_some() {
        return Ok(None);
    }
    let base = activitypub::replies_url(&state.origin, account, status);
    let replies = state
        .repository
        .activitypub_reply_statuses(account.id, status.id, false, None, 5)
        .await
        .map_err(|_| ())?;
    let mut items = Vec::new();
    for reply in &replies {
        items.push(
            reply
                .uri
                .clone()
                .map_or(serde_json::Value::Null, serde_json::Value::String),
        );
    }
    let next = replies.last().map_or_else(
        || format!("{base}?page=true&only_other_accounts=true"),
        |reply| format!("{base}?min_id={}&page=true", reply.id),
    );
    Ok(Some(serde_json::json!({
        "id": base,
        "type": "Collection",
        "first": {
            "type": "CollectionPage",
            "partOf": base,
            "items": items,
            "next": next
        }
    })))
}

pub(super) async fn federation_outbox_username(
    State(state): State<WebState>,
    Extension(metadata): Extension<RequestMetadata>,
    Path(username): Path<String>,
    Query(parameters): Query<HashMap<String, String>>,
    headers: HeaderMap,
    uri: Uri,
) -> Response<Body> {
    if !accepts_activitypub(&headers) {
        return html_redirect(&state.origin, &format!("/@{username}"));
    }
    let account_id = match federation_local_account_id(&state, &username).await {
        Ok(account_id) => account_id,
        Err(response) => return response,
    };
    federation_outbox_response(
        &state,
        account_id,
        metadata.client_ip,
        parameters,
        &headers,
        &uri,
    )
    .await
}

pub(super) async fn federation_outbox_id(
    State(state): State<WebState>,
    Extension(metadata): Extension<RequestMetadata>,
    Path(account_id): Path<String>,
    Query(parameters): Query<HashMap<String, String>>,
    headers: HeaderMap,
    uri: Uri,
) -> Response<Body> {
    let Some(account_id) = activitypub_path_id(&account_id) else {
        return not_found();
    };
    federation_outbox_response(
        &state,
        account_id,
        metadata.client_ip,
        parameters,
        &headers,
        &uri,
    )
    .await
}

pub(super) async fn federation_outbox_instance(
    State(state): State<WebState>,
    Extension(metadata): Extension<RequestMetadata>,
    Query(parameters): Query<HashMap<String, String>>,
    headers: HeaderMap,
    uri: Uri,
) -> Response<Body> {
    federation_outbox_response(&state, -99, metadata.client_ip, parameters, &headers, &uri).await
}

#[allow(
    clippy::manual_let_else,
    clippy::too_many_lines,
    clippy::uninlined_format_args
)]
pub(super) async fn federation_outbox_response(
    state: &WebState,
    account_id: i64,
    client_ip: IpAddr,
    parameters: HashMap<String, String>,
    headers: &HeaderMap,
    uri: &Uri,
) -> Response<Body> {
    if !accepts_activitypub(headers) {
        return error_response(
            StatusCode::NOT_ACCEPTABLE,
            "ActivityPub representation required",
        );
    }
    let viewer_account_id = match federation_signed_viewer(state, client_ip, uri, headers).await {
        Ok(viewer_account_id) => viewer_account_id,
        Err(response) => return response,
    };
    let account = match federation_account(state, account_id, true).await {
        Ok(account) => account,
        Err(response) => return response,
    };
    let base = Url::parse(&activitypub::collection_url(
        &state.origin,
        &account,
        "outbox",
    ))
    .expect("origin is absolute");
    if !parameters
        .get("page")
        .is_some_and(|value| activitypub_truthy(value))
    {
        let total = match state.repository.activitypub_outbox_count(account_id).await {
            Ok(total) => total,
            Err(_) => return internal_error(),
        };
        return signature_sensitive_activity_response(
            headers,
            activitypub::ordered_collection(
                base.to_string(),
                total,
                format!("{}?page=true", base),
                Some(format!("{}?min_id=0&page=true", base)),
            ),
        );
    }
    let max_id = parameters
        .get("max_id")
        .and_then(|value| route_path_id(value));
    let min_id = parameters
        .get("min_id")
        .and_then(|value| route_path_id(value));
    let since_id = parameters
        .get("since_id")
        .and_then(|value| route_path_id(value));
    let statuses = match state
        .repository
        .activitypub_outbox_statuses(account_id, viewer_account_id, 20, max_id, min_id, since_id)
        .await
    {
        Ok(statuses) => statuses,
        Err(_) => return internal_error(),
    };
    let first_status_id = statuses.first().map(|status| status.id);
    let last_status_id = statuses.last().map(|status| status.id);
    let full_page = statuses.len() == 20;
    let mut items = Vec::new();
    for status in statuses {
        let note = match state.repository.account(status.account_id).await {
            Ok(Some(status_account)) => {
                match federation_note_value(state, &status, &status_account).await {
                    Ok(note) => note,
                    Err(()) => return internal_error(),
                }
            }
            Ok(None) | Err(_) => return internal_error(),
        };
        let actor = activitypub::actor_url(&state.origin, &account);
        let item_id = if status.reblog_of_id.is_some() {
            note["id"].clone()
        } else {
            serde_json::Value::String(format!(
                "{}{}",
                note["id"].as_str().unwrap_or_default(),
                "/activity"
            ))
        };
        let item = if let Some(source_id) = status.reblog_of_id {
            let object = match state.repository.status(source_id).await {
                Ok(Some(source)) => match state.repository.account(source.account_id).await {
                    Ok(Some(source_account)) => Some(activitypub::status_uri(
                        &state.origin,
                        &source_account,
                        &source,
                    )),
                    Ok(None) | Err(_) => return internal_error(),
                },
                Ok(None) | Err(_) => return internal_error(),
            };
            let Some(object) = object else {
                return internal_error();
            };
            serde_json::json!({
                "@context": activitypub::ACTIVITY_STREAMS_CONTEXT,
                "id": item_id,
                "type": "Announce",
                "actor": actor,
                "published": note["published"],
                "to": note["to"],
                "cc": note["cc"],
                "object": object
            })
        } else {
            serde_json::json!({
                "@context": activitypub::ACTIVITY_STREAMS_CONTEXT,
                "id": item_id,
                "type": "Create",
                "actor": actor,
                "published": note["published"],
                "to": note["to"],
                "cc": note["cc"],
                "object": note
            })
        };
        items.push(item);
    }
    let id = outbox_page_url(&base, max_id, min_id, since_id);
    let next = full_page
        .then_some(last_status_id)
        .flatten()
        .map(|id| outbox_page_url(&base, Some(id), None, None));
    let prev = first_status_id.map(|id| outbox_page_url(&base, None, Some(id), None));
    signature_sensitive_activity_response(
        headers,
        activitypub::ordered_page(id, base.to_string(), None, items, next, prev),
    )
}

pub(super) fn outbox_page_url(
    base: &Url,
    max_id: Option<i64>,
    min_id: Option<i64>,
    since_id: Option<i64>,
) -> String {
    let mut query = Vec::new();
    if let Some(id) = max_id {
        query.push(format!("max_id={id}"));
    }
    if let Some(id) = min_id {
        query.push(format!("min_id={id}"));
    }
    if let Some(id) = since_id {
        query.push(format!("since_id={id}"));
    }
    query.push("page=true".to_owned());
    format!("{}?{}", base, query.join("&"))
}

pub(super) async fn federation_followers_username(
    State(state): State<WebState>,
    Extension(metadata): Extension<RequestMetadata>,
    Path(username): Path<String>,
    Query(parameters): Query<HashMap<String, String>>,
    headers: HeaderMap,
    uri: Uri,
) -> Response<Body> {
    federation_collection_username(
        &state,
        username,
        true,
        metadata.client_ip,
        parameters,
        &headers,
        &uri,
    )
    .await
}

pub(super) async fn federation_following_username(
    State(state): State<WebState>,
    Extension(metadata): Extension<RequestMetadata>,
    Path(username): Path<String>,
    Query(parameters): Query<HashMap<String, String>>,
    headers: HeaderMap,
    uri: Uri,
) -> Response<Body> {
    federation_collection_username(
        &state,
        username,
        false,
        metadata.client_ip,
        parameters,
        &headers,
        &uri,
    )
    .await
}

pub(super) async fn federation_collection_username(
    state: &WebState,
    username: String,
    followers: bool,
    client_ip: IpAddr,
    parameters: HashMap<String, String>,
    headers: &HeaderMap,
    uri: &Uri,
) -> Response<Body> {
    if !accepts_activitypub(headers) {
        let collection = if followers { "followers" } else { "following" };
        return html_redirect(&state.origin, &format!("/@{username}/{collection}"));
    }
    let account_id = match federation_local_account_id(state, &username).await {
        Ok(account_id) => account_id,
        Err(response) => return response,
    };
    federation_collection_response(
        state, account_id, followers, client_ip, parameters, headers, uri,
    )
    .await
}

pub(super) async fn federation_followers_id(
    State(state): State<WebState>,
    Extension(metadata): Extension<RequestMetadata>,
    Path(account_id): Path<String>,
    Query(parameters): Query<HashMap<String, String>>,
    headers: HeaderMap,
    uri: Uri,
) -> Response<Body> {
    let Some(account_id) = activitypub_path_id(&account_id) else {
        return not_found();
    };
    federation_collection_response(
        &state,
        account_id,
        true,
        metadata.client_ip,
        parameters,
        &headers,
        &uri,
    )
    .await
}

pub(super) async fn federation_following_id(
    State(state): State<WebState>,
    Extension(metadata): Extension<RequestMetadata>,
    Path(account_id): Path<String>,
    Query(parameters): Query<HashMap<String, String>>,
    headers: HeaderMap,
    uri: Uri,
) -> Response<Body> {
    let Some(account_id) = activitypub_path_id(&account_id) else {
        return not_found();
    };
    federation_collection_response(
        &state,
        account_id,
        false,
        metadata.client_ip,
        parameters,
        &headers,
        &uri,
    )
    .await
}

#[allow(clippy::manual_let_else, clippy::uninlined_format_args)]
pub(super) async fn federation_collection_response(
    state: &WebState,
    account_id: i64,
    followers: bool,
    client_ip: IpAddr,
    parameters: HashMap<String, String>,
    headers: &HeaderMap,
    uri: &Uri,
) -> Response<Body> {
    if !accepts_activitypub(headers) {
        return error_response(
            StatusCode::NOT_ACCEPTABLE,
            "ActivityPub representation required",
        );
    }
    if let Err(response) =
        verify_optional_federation_signature(state, client_ip, uri, headers).await
    {
        return response;
    }
    let account = match federation_account(state, account_id, true).await {
        Ok(account) => account,
        Err(response) => return response,
    };
    let route = if followers { "followers" } else { "following" };
    let base = Url::parse(&activitypub::collection_url(&state.origin, &account, route))
        .expect("origin is absolute");
    let total = match state
        .repository
        .activitypub_follow_count(account_id, followers)
        .await
    {
        Ok(total) => total,
        Err(_) => return internal_error(),
    };
    let paged = parameters.contains_key("page");
    if account.hide_collections.unwrap_or(false) && paged {
        return error_response(StatusCode::FORBIDDEN, "Collection is private");
    }
    if !paged {
        let Some(first) =
            (!account.hide_collections.unwrap_or(false)).then(|| format!("{}?page=1", base))
        else {
            return activity_response(
                StatusCode::OK,
                ACTIVITY_JSON,
                serde_json::json!({
                    "@context": activitypub::ACTIVITY_STREAMS_CONTEXT,
                    "id": base.to_string(),
                    "type": "OrderedCollection",
                    "totalItems": total
                }),
            );
        };
        return activity_response(
            StatusCode::OK,
            ACTIVITY_JSON,
            activitypub::ordered_collection(base.to_string(), total, first, None),
        );
    }
    let offset = parameters
        .get("offset")
        .and_then(|value| value.parse::<i64>().ok())
        .or_else(|| {
            parameters
                .get("page")
                .and_then(|value| value.parse::<i64>().ok())
                .map(|page| page.saturating_sub(1).saturating_mul(12))
        })
        .unwrap_or(0)
        .max(0);
    let ids = match state
        .repository
        .activitypub_follow_account_ids(account_id, followers, 12, offset)
        .await
    {
        Ok(ids) => ids,
        Err(_) => return internal_error(),
    };
    let mut items = Vec::new();
    for id in ids {
        if let Ok(Some(target)) = state.repository.account(id).await {
            items.push(serde_json::Value::String(activitypub::actor_url(
                &state.origin,
                &target,
            )));
        }
    }
    let page = offset / 12 + 1;
    let id = format!("{}?page={page}", base);
    let next = (items.len() == 12).then(|| format!("{}?page={}", base, page + 1));
    let prev = (page > 1).then(|| format!("{}?page={}", base, page - 1));
    activity_response(
        StatusCode::OK,
        ACTIVITY_JSON,
        activitypub::ordered_page(id, base.to_string(), Some(total), items, next, prev),
    )
}
