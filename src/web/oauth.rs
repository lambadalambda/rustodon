//! OAuth authorization, token, revocation and metadata endpoints.

use super::*;

pub(super) async fn oauth_token(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    headers: HeaderMap,
) -> Response<Body> {
    let grant_type = match oauth_scalar(&rack, "grant_type") {
        Ok(Some(value)) if !value.is_empty() => value,
        _ => {
            return oauth_token_error(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "Missing required parameter: grant_type.",
            );
        }
    };
    if grant_type == "authorization_code" {
        return oauth_authorization_code_token(state, &rack, &headers).await;
    }
    if grant_type != "client_credentials" {
        return oauth_token_error(
            StatusCode::BAD_REQUEST,
            "unsupported_grant_type",
            "The authorization grant type is not supported by the authorization server.",
        );
    }
    let Ok((client_id, client_secret)) = oauth_client_credentials(&rack, &headers) else {
        return oauth_token_error(
            StatusCode::UNAUTHORIZED,
            "invalid_client",
            "Client authentication failed due to unknown client, no client authentication included, or unsupported authentication method.",
        );
    };
    let Ok(requested_scope) = oauth_scalar(&rack, "scope") else {
        return oauth_token_error(
            StatusCode::BAD_REQUEST,
            "invalid_scope",
            "The requested scope is invalid, unknown, or malformed.",
        );
    };
    let Some(writer) = state.write_repository.as_ref() else {
        return internal_error();
    };
    let token = match writer
        .issue_oauth_client_credentials_token(&client_id, &client_secret, requested_scope)
        .await
    {
        Ok(token) => token,
        Err(OAuthClientCredentialsError::InvalidClient) => {
            return oauth_token_error(
                StatusCode::UNAUTHORIZED,
                "invalid_client",
                "Client authentication failed due to unknown client, no client authentication included, or unsupported authentication method.",
            );
        }
        Err(OAuthClientCredentialsError::InvalidScope) => {
            return oauth_token_error(
                StatusCode::BAD_REQUEST,
                "invalid_scope",
                "The requested scope is invalid, unknown, or malformed.",
            );
        }
        Err(OAuthClientCredentialsError::Database(_)) => return internal_error(),
    };
    let body = serde_json::json!({
        "access_token": token.access_token,
        "token_type": "Bearer",
        "scope": token.scopes,
        "created_at": token.created_at.and_utc().timestamp(),
    });
    let Ok(body) = serde_json::to_vec(&body) else {
        return internal_error();
    };
    let mut response = json_response(StatusCode::OK, body);
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
        .headers_mut()
        .insert(PRAGMA, HeaderValue::from_static("no-cache"));
    response
}

pub(super) async fn oauth_authorization_code_token(
    state: WebState,
    parameters: &RackParameters,
    headers: &HeaderMap,
) -> Response<Body> {
    let (client_id, client_secret) = if headers.contains_key(AUTHORIZATION) {
        match oauth_basic_client_credentials(headers) {
            Ok((client_id, client_secret)) => (client_id, Some(client_secret)),
            Err(()) => {
                return oauth_token_error(
                    StatusCode::UNAUTHORIZED,
                    "invalid_client",
                    "Client authentication failed due to unknown client, no client authentication included, or unsupported authentication method.",
                );
            }
        }
    } else {
        let Ok(Some(client_id)) = oauth_scalar(parameters, "client_id") else {
            return oauth_token_error(
                StatusCode::UNAUTHORIZED,
                "invalid_client",
                "Client authentication failed due to unknown client, no client authentication included, or unsupported authentication method.",
            );
        };
        let Ok(client_secret) = oauth_scalar(parameters, "client_secret") else {
            return oauth_token_error(
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "The request is missing a required parameter.",
            );
        };
        (client_id.to_owned(), client_secret.map(str::to_owned))
    };
    let Ok(Some(code)) = oauth_scalar(parameters, "code") else {
        return oauth_token_error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "The request is missing a required parameter: code.",
        );
    };
    let Ok(Some(redirect_uri)) = oauth_scalar(parameters, "redirect_uri") else {
        return oauth_token_error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "The request is missing a required parameter: redirect_uri.",
        );
    };
    let Ok(code_verifier) = oauth_scalar(parameters, "code_verifier") else {
        return oauth_token_error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "The request contains an invalid code_verifier.",
        );
    };
    let Some(writer) = state.write_repository.as_ref() else {
        return internal_error();
    };
    let token = match writer
        .issue_oauth_authorization_code_token(
            &client_id,
            client_secret.as_deref(),
            code,
            redirect_uri,
            code_verifier,
        )
        .await
    {
        Ok(token) => token,
        Err(OAuthAuthorizationCodeError::InvalidClient) => {
            return oauth_token_error(
                StatusCode::UNAUTHORIZED,
                "invalid_client",
                "Client authentication failed due to unknown client, no client authentication included, or unsupported authentication method.",
            );
        }
        Err(OAuthAuthorizationCodeError::InvalidGrant) => {
            return oauth_token_error(
                StatusCode::BAD_REQUEST,
                "invalid_grant",
                "The provided authorization grant is invalid, expired, revoked, does not match the redirection URI used in the authorization request, or was issued to another client.",
            );
        }
        Err(OAuthAuthorizationCodeError::Database(_)) => return internal_error(),
    };
    let body = serde_json::json!({
        "access_token": token.access_token,
        "token_type": "Bearer",
        "scope": token.scopes,
        "created_at": token.created_at.and_utc().timestamp(),
    });
    let Ok(body) = serde_json::to_vec(&body) else {
        return internal_error();
    };
    let mut response = json_response(StatusCode::OK, body);
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
        .headers_mut()
        .insert(PRAGMA, HeaderValue::from_static("no-cache"));
    response
}

pub(super) async fn oauth_userinfo(
    State(state): State<WebState>,
    headers: HeaderMap,
) -> Response<Body> {
    let owner = match required_viewer_owner(&state, &headers, PROFILE).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    let Some(account) = (match state.loader(None).account(owner.account_id()).await {
        Ok(account) => account,
        Err(_) => return internal_error(),
    }) else {
        return record_not_found();
    };
    let Ok(account) = state.serializer().account(&account) else {
        return internal_error();
    };
    let body = serde_json::json!({
        "iss": state.origin.as_str(),
        "sub": account.uri,
        "name": account.display_name,
        "preferred_username": account.username,
        "profile": account.url,
        "picture": account.avatar,
    });
    let Ok(body) = serde_json::to_vec(&body) else {
        return internal_error();
    };
    let mut response = json_response(StatusCode::OK, body);
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static(PRIVATE_CACHE));
    response
        .headers_mut()
        .insert(VARY, HeaderValue::from_static("Authorization, Origin"));
    response
}

pub(super) async fn oauth_metadata(State(state): State<WebState>) -> Response<Body> {
    let endpoint = |path: &str| state.origin.join(path).ok().map(|url| url.to_string());
    let Some((
        authorization_endpoint,
        token_endpoint,
        userinfo_endpoint,
        revocation_endpoint,
        app_registration_endpoint,
    )) = [
        endpoint("oauth/authorize"),
        endpoint("oauth/token"),
        endpoint("oauth/userinfo"),
        endpoint("oauth/revoke"),
        endpoint("api/v1/apps"),
    ]
    .into_iter()
    .collect::<Option<Vec<_>>>()
    .and_then(|endpoints| {
        let mut endpoints = endpoints.into_iter();
        Some((
            endpoints.next()?,
            endpoints.next()?,
            endpoints.next()?,
            endpoints.next()?,
            endpoints.next()?,
        ))
    })
    else {
        return internal_error();
    };
    let body = serde_json::json!({
        "issuer": state.origin.as_str(),
        "authorization_endpoint": authorization_endpoint,
        "token_endpoint": token_endpoint,
        "userinfo_endpoint": userinfo_endpoint,
        "revocation_endpoint": revocation_endpoint,
        "scopes_supported": OAUTH_CONFIGURED_SCOPES,
        "response_types_supported": ["code"],
        "response_modes_supported": ["query", "fragment", "form_post"],
        "grant_types_supported": ["authorization_code", "client_credentials"],
        "token_endpoint_auth_methods_supported": ["client_secret_basic", "client_secret_post"],
        "code_challenge_methods_supported": ["S256"],
        "service_documentation": "https://docs.joinmastodon.org/",
        "app_registration_endpoint": app_registration_endpoint,
    });
    let Ok(body) = serde_json::to_vec(&body) else {
        return internal_error();
    };
    let mut response = json_response(StatusCode::OK, body);
    response.headers_mut().insert(
        CACHE_CONTROL,
        HeaderValue::from_static("max-age=0, private, must-revalidate"),
    );
    response
        .headers_mut()
        .insert(VARY, HeaderValue::from_static("Origin"));
    response
}

pub(super) async fn oauth_revoke(
    State(state): State<WebState>,
    Extension(rack): Extension<RackParameters>,
    headers: HeaderMap,
) -> Response<Body> {
    let Ok((client_id, client_secret)) = oauth_client_credentials(&rack, &headers) else {
        return oauth_revoke_error();
    };
    let token = oauth_scalar(&rack, "token").ok().flatten();
    let token_type_hint = oauth_scalar(&rack, "token_type_hint").ok().flatten();
    let Some(writer) = state.write_repository.as_ref() else {
        return internal_error();
    };
    match writer
        .revoke_oauth_token(&client_id, &client_secret, token, token_type_hint)
        .await
    {
        Ok(()) => empty_json_response(),
        Err(
            OAuthTokenRevocationError::InvalidClient
            | OAuthTokenRevocationError::UnauthorizedClient,
        ) => oauth_revoke_error(),
        Err(OAuthTokenRevocationError::Database(_)) => internal_error(),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum OAuthResponseMode {
    Query,
    Fragment,
    FormPost,
}

impl OAuthResponseMode {
    fn parse(value: Option<&str>) -> Option<Self> {
        match value
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("query")
        {
            "query" => Some(Self::Query),
            "fragment" => Some(Self::Fragment),
            "form_post" => Some(Self::FormPost),
            _ => None,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Query => "query",
            Self::Fragment => "fragment",
            Self::FormPost => "form_post",
        }
    }
}

#[allow(clippy::too_many_lines)]
pub(super) async fn oauth_authorize(
    State(state): State<WebState>,
    method: Method,
    Extension(parameters): Extension<RackParameters>,
    headers: HeaderMap,
) -> Response<Body> {
    // Fail closed before login/consent for malformed or unknown modes; blank
    // scalar modes use the authorization-code flow's query default.
    let Ok(mode) = oauth_scalar(&parameters, "response_mode") else {
        return oauth_authorize_error(StatusCode::BAD_REQUEST, "invalid_request");
    };
    let Some(response_mode) = OAuthResponseMode::parse(mode) else {
        return oauth_authorize_error(StatusCode::BAD_REQUEST, "unsupported_response_mode");
    };
    let Some(session_id) = request_cookie(&headers, BROWSER_SESSION_COOKIE) else {
        return oauth_authorize_sign_in_redirect(&parameters);
    };
    let session = match state.repository.browser_session(session_id).await {
        Ok(Some(session)) => session,
        Ok(None) => return oauth_authorize_sign_in_redirect(&parameters),
        Err(_) => return internal_error(),
    };
    if !session.functional {
        return browser_redirect_response("/settings/profile");
    }
    let Ok(Some(client_id)) = oauth_scalar(&parameters, "client_id") else {
        return oauth_authorize_error(StatusCode::BAD_REQUEST, "invalid_request");
    };
    let Ok(Some(redirect_uri)) = oauth_scalar(&parameters, "redirect_uri") else {
        return oauth_authorize_error(StatusCode::BAD_REQUEST, "invalid_request");
    };
    let Ok(Some(response_type)) = oauth_scalar(&parameters, "response_type") else {
        return oauth_authorize_error(StatusCode::BAD_REQUEST, "unsupported_response_type");
    };
    let state_value = oauth_scalar(&parameters, "state").ok().flatten();
    let application = match state.repository.oauth_application_by_uid(client_id).await {
        Ok(Some(application)) => application,
        Ok(None) => return oauth_authorize_error(StatusCode::BAD_REQUEST, "invalid_client"),
        Err(_) => return internal_error(),
    };
    if response_type != "code"
        || !application
            .redirect_uri
            .split_whitespace()
            .any(|registered| registered == redirect_uri)
    {
        return oauth_authorize_error(StatusCode::BAD_REQUEST, "invalid_request");
    }
    let Ok(redirect) = Url::parse(redirect_uri) else {
        return oauth_authorize_error(StatusCode::BAD_REQUEST, "invalid_request");
    };
    // Registration and session checks have passed. Refuse unsupported transport
    // combinations before consent, denial or any grant/code mutation.
    if response_mode == OAuthResponseMode::FormPost
        && !matches!(redirect.scheme(), "http" | "https")
    {
        return browser_json_response(
            StatusCode::BAD_REQUEST,
            &serde_json::json!({
                "error": "unsupported_response_mode",
                "error_description": "form_post requires an HTTP(S) redirect_uri."
            }),
        );
    }
    if method == Method::GET {
        let (csrf_token, csrf_cookie) = browser_page_csrf(
            &headers,
            state.origin.scheme() == "https",
            &state.csrf_signing_key,
        );
        let mut response = oauth_consent_response(
            &application.name,
            client_id,
            redirect_uri,
            response_mode,
            oauth_scalar(&parameters, "scope")
                .ok()
                .flatten()
                .unwrap_or("read"),
            state_value,
            oauth_scalar(&parameters, "code_challenge").ok().flatten(),
            oauth_scalar(&parameters, "code_challenge_method")
                .ok()
                .flatten(),
            Some(&csrf_token),
        );
        if let Some(cookie) = csrf_cookie {
            append_cookie(&mut response, &cookie);
        }
        return response;
    }
    let Some(csrf_cookie) = request_browser_csrf_cookie(&headers, state.origin.scheme() == "https")
    else {
        return browser_auth_error_response(StatusCode::FORBIDDEN, "invalid_csrf_token");
    };
    let csrf_attempt = oauth_scalar(&parameters, "csrf_token").ok().flatten();
    if !valid_browser_csrf_token(csrf_cookie, &state.csrf_signing_key)
        || csrf_attempt
            .is_none_or(|attempt| !constant_time_equal(csrf_cookie.as_bytes(), attempt.as_bytes()))
    {
        return browser_auth_error_response(StatusCode::FORBIDDEN, "invalid_csrf_token");
    }
    let approved = oauth_scalar(&parameters, "approve")
        .ok()
        .flatten()
        .or_else(|| oauth_scalar(&parameters, "commit").ok().flatten())
        .is_some_and(|value| matches!(value, "1" | "true" | "Authorize" | "authorize"));
    if !approved {
        return oauth_authorize_response(
            &redirect,
            response_mode,
            state_value,
            Some("access_denied"),
            Some("The resource owner or authorization server denied the request."),
            None,
        );
    }
    let Some(writer) = state.write_repository.as_ref() else {
        return internal_error();
    };
    let (Ok(code_challenge), Ok(code_challenge_method)) = (
        oauth_scalar(&parameters, "code_challenge"),
        oauth_scalar(&parameters, "code_challenge_method"),
    ) else {
        return oauth_authorize_error(StatusCode::BAD_REQUEST, "invalid_request");
    };
    let grant = match writer
        .create_oauth_authorization_grant(
            client_id,
            session.user_id,
            redirect_uri,
            oauth_scalar(&parameters, "scope").ok().flatten(),
            code_challenge,
            code_challenge_method,
        )
        .await
    {
        Ok(grant) => grant,
        Err(OAuthAuthorizationGrantError::InvalidClient) => {
            return oauth_authorize_error(StatusCode::BAD_REQUEST, "invalid_client");
        }
        Err(OAuthAuthorizationGrantError::InvalidCodeChallenge)
            if code_challenge.is_some_and(|value| !value.trim().is_empty())
                && code_challenge_method != Some("S256") =>
        {
            // The repository has rejected issuance. Distinguish the pinned
            // method error without weakening its required/syntax PKCE checks.
            return oauth_authorize_response(
                &redirect,
                response_mode,
                state_value,
                Some("invalid_code_challenge_method"),
                Some("The code_challenge_method must be S256."),
                None,
            );
        }
        Err(
            OAuthAuthorizationGrantError::InvalidRedirectUri
            | OAuthAuthorizationGrantError::InvalidCodeChallenge,
        ) => return oauth_authorize_error(StatusCode::BAD_REQUEST, "invalid_request"),
        Err(OAuthAuthorizationGrantError::InvalidScope) => {
            return oauth_authorize_response(
                &redirect,
                response_mode,
                state_value,
                Some("invalid_scope"),
                Some("The requested scope is invalid, unknown, or malformed."),
                None,
            );
        }
        Err(OAuthAuthorizationGrantError::Database(_)) => return internal_error(),
    };
    if redirect_uri == "urn:ietf:wg:oauth:2.0:oob" {
        return html_response(StatusCode::OK, oauth_oob_document(&grant.code));
    }
    oauth_authorize_response(
        &redirect,
        response_mode,
        state_value,
        None,
        None,
        Some(&grant.code),
    )
}

pub(super) fn oauth_oob_document(code: &str) -> String {
    let content = format!(
        "<h1>Authorization code</h1><p>Copy this code into the application:</p><p><code class=\"authorization-code\">{}</code></p>",
        html_escape::encode_text(code)
    );
    rustodon_document(
        "Authorization code",
        &content,
        RustodonDocumentLayout::Compact,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn oauth_consent_response(
    application_name: &str,
    client_id: &str,
    redirect_uri: &str,
    response_mode: OAuthResponseMode,
    scope: &str,
    state: Option<&str>,
    code_challenge: Option<&str>,
    code_challenge_method: Option<&str>,
    csrf_token: Option<&str>,
) -> Response<Body> {
    let mut fields = String::new();
    for (name, value) in [("client_id", client_id), ("redirect_uri", redirect_uri)] {
        fields.push_str(&oauth_hidden_field(name, value));
    }
    fields.push_str(&oauth_hidden_field("response_type", "code"));
    fields.push_str(&oauth_hidden_field("response_mode", response_mode.as_str()));
    fields.push_str(&oauth_hidden_field("scope", scope));
    if let Some(state) = state {
        fields.push_str(&oauth_hidden_field("state", state));
    }
    if let Some(challenge) = code_challenge {
        fields.push_str(&oauth_hidden_field("code_challenge", challenge));
    }
    if let Some(method) = code_challenge_method {
        fields.push_str(&oauth_hidden_field("code_challenge_method", method));
    }
    if let Some(csrf_token) = csrf_token {
        fields.push_str(&oauth_hidden_field("csrf_token", csrf_token));
    }
    let content = format!(
        "<h1>Authorize {}</h1><p>Requested scopes: {}</p><form method=\"post\" action=\"/oauth/authorize\">{}<div class=\"button-row\"><button class=\"button button--primary\" name=\"approve\" value=\"true\" type=\"submit\">Authorize</button><button class=\"button button--secondary\" name=\"approve\" value=\"false\" type=\"submit\">Deny</button></div></form>",
        html_escape::encode_text(application_name),
        html_escape::encode_text(scope),
        fields
    );
    html_response(
        StatusCode::OK,
        rustodon_document(
            "Authorize application",
            &content,
            RustodonDocumentLayout::Compact,
        ),
    )
}

pub(super) fn oauth_authorize_error(status: StatusCode, error: &str) -> Response<Body> {
    browser_json_response(status, &serde_json::json!({ "error": error }))
}

pub(super) fn oauth_hidden_field(name: &str, value: &str) -> String {
    format!(
        "<input type=\"hidden\" name=\"{}\" value=\"{}\">",
        html_escape::encode_quoted_attribute(name),
        html_escape::encode_quoted_attribute(value)
    )
}

// The caller must validate registration and mode/transport before grant issuance.
pub(super) fn oauth_authorize_response(
    redirect: &Url,
    response_mode: OAuthResponseMode,
    state: Option<&str>,
    error: Option<&str>,
    error_description: Option<&str>,
    code: Option<&str>,
) -> Response<Body> {
    // Match pinned blank-state handling: only a successful form-post retains
    // empty/whitespace state. Do not trim nonblank opaque state values.
    let state = state.filter(|value| {
        (response_mode == OAuthResponseMode::FormPost && error.is_none())
            || !value.trim().is_empty()
    });
    let fields = [
        ("code", code),
        ("error", error),
        ("error_description", error_description),
        ("state", state),
    ]
    .into_iter()
    .filter_map(|(name, value)| value.map(|value| (name, value)))
    .collect::<Vec<_>>();
    match response_mode {
        OAuthResponseMode::FormPost => oauth_form_post_response(redirect, &fields),
        OAuthResponseMode::Query | OAuthResponseMode::Fragment => {
            let mut redirect = redirect.clone();
            if response_mode == OAuthResponseMode::Query {
                redirect.query_pairs_mut().extend_pairs(fields);
            } else {
                let fragment = url::form_urlencoded::Serializer::new(String::new())
                    .extend_pairs(fields)
                    .finish();
                redirect.set_fragment(Some(&fragment));
            }
            browser_redirect_response(redirect.as_str())
        }
    }
}

pub(super) fn oauth_form_post_response(redirect: &Url, fields: &[(&str, &str)]) -> Response<Body> {
    const SCRIPT: &str = "document.forms[0].submit();";
    let fields = fields
        .iter()
        .map(|(name, value)| oauth_hidden_field(name, value))
        .collect::<String>();
    let html = format!(
        "<!doctype html><meta charset=\"utf-8\"><title>Authorization response</title><form method=\"post\" action=\"{}\">{}<noscript><button type=\"submit\">Continue</button></noscript></form><script>{SCRIPT}</script>",
        html_escape::encode_quoted_attribute(redirect.as_str()),
        fields
    );
    // Only the static auto-submit script is executable. The validated callback
    // origin is allowed only on this response, never on login/consent pages.
    let policy = format!(
        "default-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action {}; script-src 'sha256-{}'",
        redirect.origin().ascii_serialization(),
        STANDARD.encode(Sha256::digest(SCRIPT.as_bytes()))
    );
    let Ok(policy) = HeaderValue::from_str(&policy) else {
        return internal_error();
    };
    let mut response = html_response(StatusCode::OK, html);
    response
        .headers_mut()
        .insert("content-security-policy", policy);
    response
        .headers_mut()
        .insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    response
}

pub(super) fn oauth_scalar<'a>(
    parameters: &'a RackParameters,
    name: &str,
) -> Result<Option<&'a str>, ()> {
    match parameters.get(name) {
        None | Some(RackValue::Null) => Ok(None),
        Some(RackValue::Scalar(value)) => Ok(Some(value)),
        Some(_) => Err(()),
    }
}

pub(super) fn oauth_client_credentials(
    parameters: &RackParameters,
    headers: &HeaderMap,
) -> Result<(String, String), ()> {
    if headers.contains_key(AUTHORIZATION) {
        return oauth_basic_client_credentials(headers);
    }
    let client_id = oauth_scalar(parameters, "client_id")?.ok_or(())?;
    // Parse identity here; each grant/revocation authenticates confidential clients
    // in the repository. Public clients need not supply a secret.
    let client_secret = oauth_scalar(parameters, "client_secret")?.unwrap_or_default();
    if client_id.is_empty() {
        return Err(());
    }
    Ok((client_id.to_owned(), client_secret.to_owned()))
}

pub(super) fn oauth_basic_client_credentials(headers: &HeaderMap) -> Result<(String, String), ()> {
    let mut values = headers.get_all(AUTHORIZATION).iter();
    let value = values.next().ok_or(())?;
    if values.next().is_some() {
        return Err(());
    }
    let value = value.to_str().map_err(|_| ())?;
    let (scheme, encoded) = value.split_once(' ').ok_or(())?;
    if !scheme.eq_ignore_ascii_case("Basic") || encoded.is_empty() {
        return Err(());
    }
    let decoded = STANDARD.decode(encoded).map_err(|_| ())?;
    let decoded = String::from_utf8(decoded).map_err(|_| ())?;
    let (client_id, client_secret) = decoded.split_once(':').ok_or(())?;
    if client_id.is_empty() {
        return Err(());
    }
    Ok((client_id.to_owned(), client_secret.to_owned()))
}

pub(super) fn oauth_token_error(
    status: StatusCode,
    code: &str,
    description: &str,
) -> Response<Body> {
    let body = serde_json::to_vec(&serde_json::json!({
        "error": code,
        "error_description": description,
    }))
    .expect("OAuth error response is serializable");
    let mut response = json_response(status, body);
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    let challenge = format!(
        "Bearer realm=\"Doorkeeper\", error=\"{code}\", error_description=\"{description}\""
    );
    if let Ok(value) = HeaderValue::from_str(&challenge) {
        response.headers_mut().insert(WWW_AUTHENTICATE, value);
    }
    response
}

pub(super) fn oauth_revoke_error() -> Response<Body> {
    let body = serde_json::to_vec(&serde_json::json!({
        "error": "unauthorized_client",
        "error_description": "You are not authorized to revoke this token",
    }))
    .expect("OAuth revocation error response is serializable");
    json_response(StatusCode::FORBIDDEN, body)
}

pub(super) async fn app_create(
    State(state): State<WebState>,
    Extension(metadata): Extension<RequestMetadata>,
    Extension(rack): Extension<RackParameters>,
) -> Response<Body> {
    if let Err(limited) = state
        .oauth_application_limiter
        .check_shared(state.shared_rate_limiter.as_ref(), metadata.client_ip)
        .await
    {
        return rate_limited_response(limited);
    }
    let Some(writer) = state.write_repository.as_ref() else {
        return internal_error();
    };
    let Ok(Some(name)) = app_string_parameter(&rack, "client_name") else {
        return error_response(StatusCode::UNPROCESSABLE_ENTITY, "Validation failed");
    };
    let Ok(Some(redirect_uri)) = app_redirect_uri_parameter(&rack) else {
        return error_response(StatusCode::UNPROCESSABLE_ENTITY, "Validation failed");
    };
    let Ok(scopes) = app_scopes_parameter(&rack) else {
        return error_response(StatusCode::UNPROCESSABLE_ENTITY, "Validation failed");
    };
    let Ok(website) = app_string_parameter(&rack, "website") else {
        return error_response(StatusCode::UNPROCESSABLE_ENTITY, "Validation failed");
    };
    let registration = crate::mastodon::OAuthApplicationRegistration {
        name,
        redirect_uri,
        scopes,
        website,
    };
    let result = match writer.register_oauth_application(&registration).await {
        Ok(result) => result,
        Err(WriteError::Validation(message)) => {
            return error_response(StatusCode::UNPROCESSABLE_ENTITY, message);
        }
        Err(_) => return internal_error(),
    };
    let application = result.application;
    let redirect_uris = application
        .redirect_uri
        .split_whitespace()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let body = serde_json::json!({
        "id": application.id.to_string(),
        "name": application.name,
        "website": application.website.as_deref().filter(|website| !website.trim().is_empty()),
        "scopes": application.scopes.split_whitespace().collect::<Vec<_>>(),
        "redirect_uris": redirect_uris,
        "vapid_key": state.instance_runtime.vapid_public_key,
        "redirect_uri": application.redirect_uri,
        "client_id": application.uid,
        "client_secret": result.client_secret,
        "client_secret_expires_at": 0,
    });
    match serde_json::to_vec(&body) {
        Ok(body) => json_response(StatusCode::OK, body),
        Err(_) => internal_error(),
    }
}

pub(super) async fn app_verify_credentials(
    State(state): State<WebState>,
    headers: HeaderMap,
) -> Response<Body> {
    let authenticated = match state.authenticator.authenticate(&headers, NO_SCOPE).await {
        Ok(authenticated) => authenticated,
        Err(OAuthAuthenticationError::OAuth(error)) => {
            return error.into_http_response().map(Body::from);
        }
        Err(OAuthAuthenticationError::Repository(_)) => return internal_error(),
    };
    let Some(application_id) = authenticated.application_id() else {
        return OAuthError::Unauthenticated
            .into_http_response()
            .map(Body::from);
    };
    let application = match state.repository.oauth_application(application_id).await {
        Ok(Some(application)) => application,
        Ok(None) => {
            return OAuthError::Unauthenticated
                .into_http_response()
                .map(Body::from);
        }
        Err(_) => return internal_error(),
    };
    let redirect_uris = application
        .redirect_uri
        .split_whitespace()
        .collect::<Vec<_>>();
    let redirect_uri = redirect_uris.first().copied();
    let body = serde_json::json!({
        "id": application.id.to_string(),
        "name": application.name,
        "website": application.website.as_deref().filter(|website| !website.trim().is_empty()),
        "scopes": application.scopes.split_whitespace().collect::<Vec<_>>(),
        "redirect_uris": redirect_uris,
        "vapid_key": state.instance_runtime.vapid_public_key,
        "redirect_uri": redirect_uri,
    });
    match serde_json::to_vec(&body) {
        Ok(body) => json_response(StatusCode::OK, body),
        Err(_) => internal_error(),
    }
}

pub(super) fn app_string_parameter(
    parameters: &RackParameters,
    name: &str,
) -> Result<Option<String>, ()> {
    match parameters.get(name) {
        None | Some(RackValue::Null) => Ok(None),
        Some(RackValue::Scalar(value)) => Ok(Some(value.clone())),
        Some(_) => Err(()),
    }
}

pub(super) fn app_redirect_uri_parameter(
    parameters: &RackParameters,
) -> Result<Option<String>, ()> {
    match parameters.get("redirect_uris") {
        None | Some(RackValue::Null) => Ok(None),
        Some(RackValue::Scalar(value)) => Ok(Some(value.clone())),
        Some(RackValue::Array(_)) => {
            let values = rack_array_values(parameters, "redirect_uris");
            if values.is_empty() {
                Ok(None)
            } else {
                Ok(Some(values.join("\n")))
            }
        }
        Some(_) => Err(()),
    }
}

pub(super) fn app_scopes_parameter(parameters: &RackParameters) -> Result<String, ()> {
    match parameters.get("scopes") {
        None | Some(RackValue::Null | RackValue::Array(_)) => Ok("read".to_owned()),
        Some(RackValue::Scalar(value)) if !value.trim().is_empty() => Ok(value.clone()),
        Some(_) => Err(()),
    }
}
