use http::HeaderMap;
use http::header::{AUTHORIZATION, CACHE_CONTROL, CONTENT_TYPE, HeaderValue, WWW_AUTHENTICATE};
use rustodon::mastodon::{
    BearerToken, InvalidTokenReason, OAuthError, OAuthScopes, READ_ACCOUNTS, READ_STATUSES,
    VERIFY_CREDENTIALS,
};

fn authorization(value: &'static str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(AUTHORIZATION, HeaderValue::from_static(value));
    headers
}

#[test]
fn bearer_scheme_is_case_insensitive_but_requires_the_standard_separator() {
    for value in ["Bearer secret", "bearer secret", "BEARER secret"] {
        BearerToken::from_headers(&authorization(value)).expect("standard bearer header");
    }

    for value in ["Bearer", "Bearer\tsecret", " Basic secret", "Bearer   "] {
        let error = BearerToken::from_headers(&authorization(value))
            .expect_err("missing or malformed bearer credential");
        assert_eq!(error, OAuthError::Unauthenticated);
    }

    let mut duplicate = authorization("Bearer first-secret");
    duplicate.append(
        AUTHORIZATION,
        HeaderValue::from_static("Bearer second-secret"),
    );
    assert_eq!(
        BearerToken::from_headers(&duplicate).expect_err("ambiguous credentials must be rejected"),
        OAuthError::Unauthenticated
    );
}

#[test]
fn bearer_token_debug_output_is_always_redacted() {
    let token =
        BearerToken::from_headers(&authorization("Bearer fixture-sensitive-prefix-and-suffix"))
            .expect("valid bearer header");
    let diagnostic = format!("{token:?}");

    assert_eq!(diagnostic, "BearerToken([REDACTED])");
    assert!(!diagnostic.contains("fixture-sensitive"));
    assert!(!diagnostic.contains("prefix"));
    assert!(!diagnostic.contains("suffix"));
}

#[test]
fn scopes_are_exact_endpoint_specific_or_alternatives() {
    let broad = OAuthScopes::parse(Some("read write read"));
    assert!(broad.permits(READ_ACCOUNTS));
    assert!(broad.permits(READ_STATUSES));

    let accounts = OAuthScopes::parse(Some("push\tread:accounts read:accounts"));
    assert!(accounts.permits(READ_ACCOUNTS));
    assert!(!accounts.permits(READ_STATUSES));

    let statuses = OAuthScopes::parse(Some("read:statuses"));
    assert!(statuses.permits(READ_STATUSES));
    assert!(!statuses.permits(READ_ACCOUNTS));

    let profile = OAuthScopes::parse(Some("profile"));
    assert!(profile.permits(VERIFY_CREDENTIALS));
    assert!(!profile.permits(READ_ACCOUNTS));
    assert!(!OAuthScopes::parse(None).permits(READ_ACCOUNTS));
    assert!(!OAuthScopes::parse(Some("READ")).permits(READ_ACCOUNTS));
}

#[test]
fn invalid_token_responses_match_mastodon_without_exposing_credentials() {
    for (reason, message) in [
        (InvalidTokenReason::Unknown, "The access token is invalid"),
        (InvalidTokenReason::Revoked, "The access token was revoked"),
        (InvalidTokenReason::Expired, "The access token expired"),
    ] {
        let response = OAuthError::InvalidToken(reason).into_http_response();
        assert_eq!(response.status(), 401);
        assert_eq!(
            response.headers()[CONTENT_TYPE],
            "application/json; charset=utf-8"
        );
        assert_eq!(response.headers()[CACHE_CONTROL], "private, no-store");
        assert_eq!(
            response.headers()[WWW_AUTHENTICATE],
            format!(
                "Bearer realm=\"Doorkeeper\", error=\"invalid_token\", error_description=\"{message}\""
            )
        );
        assert_eq!(
            response.body(),
            format!(r#"{{"error":"{message}"}}"#).as_bytes()
        );
    }

    let unauthenticated = OAuthError::Unauthenticated.into_http_response();
    let unknown = OAuthError::InvalidToken(InvalidTokenReason::Unknown).into_http_response();
    assert_eq!(unauthenticated.status(), unknown.status());
    assert_eq!(unauthenticated.headers(), unknown.headers());
    assert_eq!(unauthenticated.body(), unknown.body());
}

#[test]
fn scope_and_owner_failures_have_stable_mastodon_http_responses() {
    let wrong_scope = OAuthError::InsufficientScope(VERIFY_CREDENTIALS).into_http_response();
    assert_eq!(wrong_scope.status(), 403);
    assert_eq!(
        wrong_scope.body(),
        br#"{"error":"This action is outside the authorized scopes"}"#
    );
    assert_eq!(
        wrong_scope.headers()[WWW_AUTHENTICATE],
        "Bearer realm=\"Doorkeeper\", error=\"insufficient_scope\", error_description=\"Access to this resource requires scope _profile read read:accounts_.\""
    );

    for (error, status, body) in [
        (
            OAuthError::UserRequired,
            422,
            r#"{"error":"This method requires an authenticated user"}"#,
        ),
        (
            OAuthError::EmailUnconfirmed,
            403,
            r#"{"error":"Your login is missing a confirmed e-mail address"}"#,
        ),
        (
            OAuthError::PendingApproval,
            403,
            r#"{"error":"Your login is currently pending approval"}"#,
        ),
        (
            OAuthError::UserDisabled,
            403,
            r#"{"error":"Your login is currently disabled"}"#,
        ),
    ] {
        let response = error.into_http_response();
        assert_eq!(response.status(), status);
        assert_eq!(response.body(), body.as_bytes());
        assert!(!response.headers().contains_key(WWW_AUTHENTICATE));
    }
}
