//! Admin-authored instance Markdown, not the post/profile HTML formatter.
//! Compatibility scope and source provenance: docs/extended-description.md.
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd, html};

use super::{
    AUTHORIZATION, BROWSER_SESSION_COOKIE, Body, HeaderMap, HeaderValue, NO_SCOPE, NaiveDateTime,
    OAuthAuthenticationError, OAuthError, Response, SecondsFormat, State, StatusCode, WebState,
    error_response, internal_error, json_response, request_cookie, required_viewer_owner,
};
use crate::mastodon::RawYamlText;

pub(super) async fn show(State(state): State<WebState>, headers: HeaderMap) -> Response<Body> {
    // Instances::BaseController skips require_authenticated_user! outside limited
    // mode; this controller also hides current_user there (even for stale tokens).
    if state.instance_runtime.limited_federation
        && let Err(response) = require_instance_user(&state, &headers).await
    {
        return response;
    }
    let Ok(setting) = state.repository.extended_description_setting().await else {
        return internal_error();
    };
    let value = setting.as_ref().and_then(|setting| setting.value.as_ref());
    let updated_at = setting.as_ref().and_then(|setting| setting.updated_at);
    match description(value, updated_at)
        .and_then(|value| serde_json::to_vec(&value).map_err(|_| ()))
    {
        Ok(body) => json_response(StatusCode::OK, body),
        Err(()) => internal_error(),
    }
}

async fn require_instance_user(
    state: &WebState,
    headers: &HeaderMap,
) -> Result<(), Response<Body>> {
    match state.authenticator.authenticate(headers, NO_SCOPE).await {
        Ok(bearer) if bearer.resource_owner().is_some() => {
            return bearer
                .require_user()
                .map(|_| ())
                .map_err(|error| error.into_http_response().map(Body::from));
        }
        Err(OAuthAuthenticationError::Repository(_)) => return Err(internal_error()),
        Err(OAuthAuthenticationError::OAuth(error))
            if !matches!(
                error,
                OAuthError::Unauthenticated | OAuthError::InvalidToken(_)
            ) =>
        {
            return Err(error.into_http_response().map(Body::from));
        }
        _ => {}
    }
    // Api::BaseController#current_user falls back to the browser session when
    // no token resource owner exists. Reuse the existing session/token lifecycle
    // checks, including confirmed/approved/disabled/moved/required-2FA states.
    if let Some(session_id) = request_cookie(headers, BROWSER_SESSION_COOKIE) {
        let session = state
            .repository
            .browser_session(session_id)
            .await
            .map_err(|_| internal_error())?;
        if let Some(session) = session {
            let mut session_headers = HeaderMap::new();
            session_headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {}", session.access_token.as_str()))
                    .map_err(|_| internal_error())?,
            );
            return required_viewer_owner(state, &session_headers, NO_SCOPE)
                .await
                .map(|_| ());
        }
    }
    Err(error_response(
        StatusCode::UNAUTHORIZED,
        "This method requires an authenticated user",
    ))
}

pub(super) fn description(
    value: Option<&RawYamlText>,
    updated_at: Option<NaiveDateTime>,
) -> Result<serde_json::Value, ()> {
    let empty = || serde_json::json!({"updated_at": null, "content": ""});
    let Some(value) = value else {
        return Ok(empty());
    };
    let documents = value.parse().map_err(|_| ())?;
    let document = match documents.as_slice() {
        [] => return Ok(empty()),
        [document] => document,
        _ => return Err(()),
    };
    if document.is_null() || document.as_bool() == Some(false) {
        return Ok(empty());
    }
    // Do not turn corrupt/non-string configuration into a misleading empty page.
    let text = document.as_str().ok_or(())?;
    if text.trim().is_empty() {
        return Ok(empty());
    }
    Ok(serde_json::json!({
        "updated_at": updated_at.map(|value| value.and_utc().to_rfc3339_opts(SecondsFormat::Secs, false)),
        "content": render_markdown(text),
    }))
}

pub(super) fn render_markdown(text: &str) -> String {
    let mut previous_block_end = false;
    let events = Parser::new_ext(text, Options::empty()).flat_map(|event| {
        // Redcarpet separates sibling blocks with a blank line. Adapt parser
        // events, never replace substrings of rendered/trusted HTML or code.
        let starts_block = matches!(
            event,
            Event::Start(
                Tag::Paragraph
                    | Tag::Heading { .. }
                    | Tag::BlockQuote(_)
                    | Tag::CodeBlock(_)
                    | Tag::List(_)
                    | Tag::HtmlBlock
            ) | Event::Rule
        );
        let separator = (previous_block_end && starts_block).then(|| Event::Html("\n".into()));
        previous_block_end = matches!(
            event,
            Event::End(
                TagEnd::Paragraph
                    | TagEnd::Heading(_)
                    | TagEnd::BlockQuote(_)
                    | TagEnd::CodeBlock
                    | TagEnd::List(_)
                    | TagEnd::HtmlBlock
            ) | Event::Rule
        );
        let event = match event {
            Event::HardBreak => Event::Html("<br>\n".into()),
            Event::Rule => Event::Html("<hr>\n".into()),
            event => event,
        };
        separator.into_iter().chain(std::iter::once(event))
    });
    let mut content = String::new();
    html::push_html(&mut content, events);
    content
}
