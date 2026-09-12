//! The pinned web client saves whole snapshots; this is not the preferences API.
use serde_json::{Map, Value};

use super::{
    AUTHORIZATION, BROWSER_SESSION_COOKIE, Body, Extension, HeaderMap, HeaderValue, NO_SCOPE,
    RackParameters, RackValue, Response, State, StatusCode, WebState, WriteError,
    constant_time_equal, empty_json_response, error_response, internal_error, not_found,
    request_browser_csrf_cookie, request_cookie, required_viewer_owner, valid_browser_csrf_token,
};

pub(super) async fn update(
    State(state): State<WebState>,
    Extension(parameters): Extension<RackParameters>,
    mut headers: HeaderMap,
) -> Response<Body> {
    // Unlike ordinary OAuth APIs, Api::Web::BaseController requires CSRF even
    // when the frontend's bootstrap bearer token is supplied.
    let cookie = request_browser_csrf_cookie(&headers, state.origin.scheme() == "https");
    let attempt = headers
        .get("x-csrf-token")
        .and_then(|value| value.to_str().ok());
    if !cookie.zip(attempt).is_some_and(|(cookie, attempt)| {
        valid_browser_csrf_token(cookie, &state.csrf_signing_key)
            && constant_time_equal(cookie.as_bytes(), attempt.as_bytes())
    }) {
        return error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Can't verify CSRF token authenticity.",
        );
    }
    // Bearer identity takes precedence, but never treat an invalid bearer as a
    // different signed-in user. Session-only requests use the existing token and
    // user lifecycle checks rather than introducing a second authentication path.
    if !headers.contains_key(AUTHORIZATION) {
        let session = match request_cookie(&headers, BROWSER_SESSION_COOKIE) {
            Some(id) => match state.repository.browser_session(id).await {
                Ok(session) => session,
                Err(_) => return internal_error(),
            },
            None => None,
        };
        let Some(session) = session else {
            return error_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                "This method requires an authenticated user",
            );
        };
        let Ok(authorization) =
            HeaderValue::from_str(&format!("Bearer {}", session.access_token.as_str()))
        else {
            return internal_error();
        };
        headers.insert(AUTHORIZATION, authorization);
    }
    let owner = match required_viewer_owner(&state, &headers, NO_SCOPE).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    let Some(data) = parameters
        .0
        .get("data")
        .filter(|data| matches!(data, RackValue::Object(_)))
        .and_then(json_value)
    else {
        return error_response(StatusCode::UNPROCESSABLE_ENTITY, "data must be an object");
    };
    let Some(writer) = state.write_repository.as_ref() else {
        return internal_error();
    };
    match writer
        .update_web_settings(owner.user_id(), owner.account_id(), &data)
        .await
    {
        Ok(()) => empty_json_response(),
        Err(WriteError::NotFound) => not_found(),
        Err(WriteError::Unauthorized) => {
            error_response(StatusCode::FORBIDDEN, "Your login is currently disabled")
        }
        Err(_) => internal_error(),
    }
}

// Preserve JSON types and Rack form strings; uploads aren't settings values.
fn json_value(value: &RackValue) -> Option<Value> {
    match value {
        RackValue::Null => Some(Value::Null),
        RackValue::Scalar(value) => Some(Value::String(value.clone())),
        RackValue::Number(value) => Some(Value::Number(value.clone())),
        RackValue::Boolean(value) => Some(Value::Bool(*value)),
        RackValue::Array(values) => values
            .iter()
            .map(json_value)
            .collect::<Option<Vec<_>>>()
            .map(Value::Array),
        RackValue::Object(values) => values
            .iter()
            .map(|(key, value)| Some((key.clone(), json_value(value)?)))
            .collect::<Option<Map<_, _>>>()
            .map(Value::Object),
        RackValue::Upload(_) => None,
    }
}
