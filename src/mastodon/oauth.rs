use std::collections::BTreeSet;
use std::fmt;

use chrono::{NaiveDateTime, TimeDelta, Utc};
use http::header::{
    AUTHORIZATION, CACHE_CONTROL, CONTENT_TYPE, HeaderName, HeaderValue, VARY, WWW_AUTHENTICATE,
};
use http::{HeaderMap, Response, StatusCode};
use zeroize::Zeroizing;

use super::Repository;
use super::policy::{AccountLifecycle, AccountLifecycleFacts, AccountSuspension};
use super::records::OAuthBearerCandidate;

const APPLICATION_JSON: HeaderValue = HeaderValue::from_static("application/json; charset=utf-8");
const AUTHORIZATION_HEADER: HeaderValue = HeaderValue::from_static("Authorization");
const OAUTH_CACHE_CONTROL: HeaderValue = HeaderValue::from_static("private, no-store");
const USER_CACHE_CONTROL: HeaderValue = HeaderValue::from_static("private, no-store");

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RequiredScopes(&'static [&'static str]);

impl RequiredScopes {
    const fn new(scopes: &'static [&'static str]) -> Self {
        Self(scopes)
    }

    #[must_use]
    pub const fn as_slice(self) -> &'static [&'static str] {
        self.0
    }
}

pub const READ_ACCOUNTS: RequiredScopes = RequiredScopes::new(&["read", "read:accounts"]);
pub const READ_BLOCKS: RequiredScopes = RequiredScopes::new(&["follow", "read", "read:blocks"]);
pub const READ_BOOKMARKS: RequiredScopes = RequiredScopes::new(&["read", "read:bookmarks"]);
pub const READ_COLLECTIONS: RequiredScopes = RequiredScopes::new(&["read", "read:collections"]);
pub const READ_FAVOURITES: RequiredScopes = RequiredScopes::new(&["read", "read:favourites"]);
pub const READ_FOLLOWS: RequiredScopes = RequiredScopes::new(&["read", "read:follows"]);
pub const READ_FILTERS: RequiredScopes = RequiredScopes::new(&["read", "read:filters"]);
pub const READ_LISTS: RequiredScopes = RequiredScopes::new(&["read", "read:lists"]);
pub const READ_MUTES: RequiredScopes = RequiredScopes::new(&["follow", "read", "read:mutes"]);
pub const READ_NOTIFICATIONS: RequiredScopes = RequiredScopes::new(&["read", "read:notifications"]);
pub const READ_STATUSES: RequiredScopes = RequiredScopes::new(&["read", "read:statuses"]);
pub const NO_SCOPE: RequiredScopes = RequiredScopes::new(&[]);
pub const VERIFY_CREDENTIALS: RequiredScopes =
    RequiredScopes::new(&["profile", "read", "read:accounts"]);

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct OAuthScopes(BTreeSet<String>);

impl OAuthScopes {
    #[must_use]
    pub fn parse(scopes: Option<&str>) -> Self {
        Self(
            scopes
                .into_iter()
                .flat_map(str::split_whitespace)
                .map(str::to_owned)
                .collect(),
        )
    }

    #[must_use]
    pub fn permits(&self, required: RequiredScopes) -> bool {
        if required.as_slice().is_empty() {
            return true;
        }
        required
            .as_slice()
            .iter()
            .any(|scope| self.0.contains(*scope))
    }

    #[must_use]
    pub fn contains(&self, scope: &str) -> bool {
        self.0.contains(scope)
    }
}

pub struct BearerToken(Zeroizing<String>);

impl BearerToken {
    /// Extracts one standard bearer credential from the authorization headers.
    ///
    /// # Errors
    ///
    /// Returns [`OAuthError::Unauthenticated`] when the header is missing,
    /// malformed, empty, non-UTF-8, or repeated.
    pub fn from_headers(headers: &HeaderMap) -> Result<Self, OAuthError> {
        let mut values = headers.get_all(AUTHORIZATION).iter();
        let value = values.next().ok_or(OAuthError::Unauthenticated)?;
        if values.next().is_some() {
            return Err(OAuthError::Unauthenticated);
        }
        let value = value.to_str().map_err(|_| OAuthError::Unauthenticated)?;
        let Some((scheme, token)) = value.get(..7).zip(value.get(7..)) else {
            return Err(OAuthError::Unauthenticated);
        };
        if !scheme.eq_ignore_ascii_case("Bearer ") || token.trim().is_empty() {
            return Err(OAuthError::Unauthenticated);
        }
        Ok(Self(Zeroizing::new(token.to_owned())))
    }

    fn secret(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Debug for BearerToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("BearerToken([REDACTED])")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InvalidTokenReason {
    Unknown,
    Revoked,
    Expired,
}

impl InvalidTokenReason {
    const fn message(self) -> &'static str {
        match self {
            Self::Unknown => "The access token is invalid",
            Self::Revoked => "The access token was revoked",
            Self::Expired => "The access token expired",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OAuthError {
    Unauthenticated,
    InvalidToken(InvalidTokenReason),
    InsufficientScope(RequiredScopes),
    UserRequired,
    EmailUnconfirmed,
    PendingApproval,
    UserDisabled,
}

impl OAuthError {
    #[must_use]
    pub fn into_http_response(self) -> Response<Vec<u8>> {
        match self {
            Self::Unauthenticated | Self::InvalidToken(InvalidTokenReason::Unknown) => {
                invalid_token_response(InvalidTokenReason::Unknown)
            }
            Self::InvalidToken(reason) => invalid_token_response(reason),
            Self::InsufficientScope(required) => {
                let description = format!(
                    "Access to this resource requires scope _{}_.",
                    required.as_slice().join(" ")
                );
                oauth_error_response(
                    StatusCode::FORBIDDEN,
                    "This action is outside the authorized scopes",
                    &format!(
                        "Bearer realm=\"Doorkeeper\", error=\"insufficient_scope\", error_description=\"{description}\""
                    ),
                )
            }
            Self::UserRequired => user_error_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                "This method requires an authenticated user",
            ),
            Self::EmailUnconfirmed => user_error_response(
                StatusCode::FORBIDDEN,
                "Your login is missing a confirmed e-mail address",
            ),
            Self::PendingApproval => user_error_response(
                StatusCode::FORBIDDEN,
                "Your login is currently pending approval",
            ),
            Self::UserDisabled => {
                user_error_response(StatusCode::FORBIDDEN, "Your login is currently disabled")
            }
        }
    }
}

impl fmt::Display for OAuthError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::Unauthenticated | Self::InvalidToken(InvalidTokenReason::Unknown) => {
                InvalidTokenReason::Unknown.message()
            }
            Self::InvalidToken(reason) => reason.message(),
            Self::InsufficientScope(_) => "This action is outside the authorized scopes",
            Self::UserRequired => "This method requires an authenticated user",
            Self::EmailUnconfirmed => "Your login is missing a confirmed e-mail address",
            Self::PendingApproval => "Your login is currently pending approval",
            Self::UserDisabled => "Your login is currently disabled",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for OAuthError {}

#[derive(Debug)]
pub enum OAuthAuthenticationError {
    OAuth(OAuthError),
    Repository(sqlx::Error),
}

impl fmt::Display for OAuthAuthenticationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OAuth(error) => error.fmt(formatter),
            Self::Repository(_) => formatter.write_str("OAuth token lookup failed"),
        }
    }
}

impl std::error::Error for OAuthAuthenticationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::OAuth(error) => Some(error),
            Self::Repository(error) => Some(error),
        }
    }
}

impl From<OAuthError> for OAuthAuthenticationError {
    fn from(error: OAuthError) -> Self {
        Self::OAuth(error)
    }
}

impl From<sqlx::Error> for OAuthAuthenticationError {
    fn from(error: sqlx::Error) -> Self {
        Self::Repository(error)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OAuthResourceOwner {
    user_id: i64,
    account_id: i64,
    state: OAuthResourceOwnerState,
}

impl OAuthResourceOwner {
    #[must_use]
    pub const fn user_id(self) -> i64 {
        self.user_id
    }

    #[must_use]
    pub const fn account_id(self) -> i64 {
        self.account_id
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OAuthResourceOwnerState {
    Functional,
    EmailUnconfirmed,
    PendingApproval,
    Disabled,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthenticatedBearer {
    token_id: i64,
    application_id: Option<i64>,
    resource_owner: Option<OAuthResourceOwner>,
    scopes: OAuthScopes,
}

impl AuthenticatedBearer {
    #[must_use]
    pub const fn token_id(&self) -> i64 {
        self.token_id
    }

    #[must_use]
    pub const fn application_id(&self) -> Option<i64> {
        self.application_id
    }

    #[must_use]
    pub const fn resource_owner(&self) -> Option<OAuthResourceOwner> {
        self.resource_owner
    }

    /// Returns the token's resource owner for endpoints that require a user.
    ///
    /// # Errors
    ///
    /// Returns the matching Mastodon owner-state error for application-only or
    /// non-functional owners.
    pub const fn require_user(&self) -> Result<OAuthResourceOwner, OAuthError> {
        match self.resource_owner {
            Some(owner) => match owner.state {
                OAuthResourceOwnerState::Functional => Ok(owner),
                OAuthResourceOwnerState::EmailUnconfirmed => Err(OAuthError::EmailUnconfirmed),
                OAuthResourceOwnerState::PendingApproval => Err(OAuthError::PendingApproval),
                OAuthResourceOwnerState::Disabled => Err(OAuthError::UserDisabled),
            },
            None => Err(OAuthError::UserRequired),
        }
    }

    #[must_use]
    pub const fn scopes(&self) -> &OAuthScopes {
        &self.scopes
    }
}

#[derive(Clone)]
pub struct BearerAuthenticator {
    repository: Repository,
}

impl BearerAuthenticator {
    #[must_use]
    pub const fn new(repository: Repository) -> Self {
        Self { repository }
    }

    /// Authenticates one bearer token at the current UTC time.
    ///
    /// # Errors
    ///
    /// Returns an OAuth failure for rejected credentials or a repository
    /// failure when `PostgreSQL` cannot complete the read-only lookup.
    pub async fn authenticate(
        &self,
        headers: &HeaderMap,
        required: RequiredScopes,
    ) -> Result<AuthenticatedBearer, OAuthAuthenticationError> {
        let token = BearerToken::from_headers(headers)?;
        let candidate = self
            .repository
            .oauth_bearer_candidate(token.secret())
            .await?
            .ok_or(OAuthError::InvalidToken(InvalidTokenReason::Unknown))?;
        authorize_candidate(&candidate, required, Utc::now().naive_utc()).map_err(Into::into)
    }
}

fn authorize_candidate(
    candidate: &OAuthBearerCandidate,
    required: RequiredScopes,
    now: NaiveDateTime,
) -> Result<AuthenticatedBearer, OAuthError> {
    if candidate.application_id.is_some() && !candidate.application_exists {
        return Err(OAuthError::InvalidToken(InvalidTokenReason::Unknown));
    }

    let suspension = if candidate.suspended_at.is_none() || candidate.account_id == Some(-99) {
        AccountSuspension::None
    } else if candidate.has_deletion_request {
        AccountSuspension::Temporary
    } else {
        AccountSuspension::Permanent
    };
    let account_lifecycle = AccountLifecycleFacts {
        suspension,
        silenced: false,
        moved: candidate.moved_to_account_id.is_some(),
        memorial: candidate.memorial == Some(true),
    };
    if candidate.user_id.is_some() && AccountLifecycle::classify(account_lifecycle).unavailable() {
        return Err(OAuthError::UserDisabled);
    }
    if candidate
        .revoked_at
        .is_some_and(|revoked_at| revoked_at <= now)
    {
        return Err(OAuthError::InvalidToken(InvalidTokenReason::Revoked));
    }
    if candidate.expires_in.is_some_and(|expires_in| {
        candidate
            .created_at
            .checked_add_signed(TimeDelta::seconds(i64::from(expires_in)))
            .is_some_and(|expires_at| now > expires_at)
    }) {
        return Err(OAuthError::InvalidToken(InvalidTokenReason::Expired));
    }

    let scopes = OAuthScopes::parse(candidate.scopes.as_deref());
    if !scopes.permits(required) {
        return Err(OAuthError::InsufficientScope(required));
    }

    let resource_owner = match (
        candidate.resource_owner_id,
        candidate.user_id,
        candidate.user_account_id,
        candidate.account_id,
    ) {
        (Some(resource_owner_id), Some(user_id), Some(user_account_id), Some(account_id))
            if resource_owner_id == user_id && user_account_id == account_id =>
        {
            let missing_required_2fa = candidate.role_requires_2fa == Some(true)
                && candidate.otp_required_for_login != Some(true)
                && !candidate.has_webauthn_credentials;
            let state = if candidate.confirmed_at.is_none() {
                OAuthResourceOwnerState::EmailUnconfirmed
            } else if candidate.approved != Some(true) {
                OAuthResourceOwnerState::PendingApproval
            } else if candidate.disabled != Some(false)
                || !account_lifecycle.functional_access_allowed()
                || missing_required_2fa
            {
                OAuthResourceOwnerState::Disabled
            } else {
                OAuthResourceOwnerState::Functional
            };
            Some(OAuthResourceOwner {
                user_id,
                account_id,
                state,
            })
        }
        _ => None,
    };

    Ok(AuthenticatedBearer {
        token_id: candidate.token_id,
        application_id: candidate.application_id,
        resource_owner,
        scopes,
    })
}

fn invalid_token_response(reason: InvalidTokenReason) -> Response<Vec<u8>> {
    oauth_error_response(
        StatusCode::UNAUTHORIZED,
        reason.message(),
        &format!(
            "Bearer realm=\"Doorkeeper\", error=\"invalid_token\", error_description=\"{}\"",
            reason.message()
        ),
    )
}

fn oauth_error_response(status: StatusCode, message: &str, challenge: &str) -> Response<Vec<u8>> {
    error_response(
        status,
        message,
        OAUTH_CACHE_CONTROL,
        Some((WWW_AUTHENTICATE, challenge)),
    )
}

fn user_error_response(status: StatusCode, message: &str) -> Response<Vec<u8>> {
    error_response(status, message, USER_CACHE_CONTROL, None)
}

fn error_response(
    status: StatusCode,
    message: &str,
    cache_control: HeaderValue,
    extra_header: Option<(HeaderName, &str)>,
) -> Response<Vec<u8>> {
    let mut builder = Response::builder()
        .status(status)
        .header(CONTENT_TYPE, APPLICATION_JSON)
        .header(CACHE_CONTROL, cache_control)
        .header(VARY, AUTHORIZATION_HEADER);
    if let Some((name, value)) = extra_header {
        builder = builder.header(name, value);
    }
    builder
        .body(format!(r#"{{"error":"{message}"}}"#).into_bytes())
        .expect("static OAuth response headers are valid")
}

#[cfg(test)]
mod tests {
    use chrono::TimeDelta;

    use super::*;

    fn candidate() -> OAuthBearerCandidate {
        OAuthBearerCandidate {
            token_id: 401,
            resource_owner_id: Some(101),
            application_id: Some(301),
            scopes: Some("read".to_owned()),
            expires_in: None,
            created_at: NaiveDateTime::parse_from_str("2026-07-01 12:00:00", "%Y-%m-%d %H:%M:%S")
                .expect("fixed timestamp"),
            revoked_at: None,
            application_exists: true,
            user_id: Some(101),
            user_account_id: Some(1_001),
            confirmed_at: Some(
                NaiveDateTime::parse_from_str("2026-07-01 12:00:00", "%Y-%m-%d %H:%M:%S")
                    .expect("fixed timestamp"),
            ),
            approved: Some(true),
            disabled: Some(false),
            otp_required_for_login: Some(true),
            role_requires_2fa: Some(true),
            has_webauthn_credentials: false,
            account_id: Some(1_001),
            suspended_at: None,
            has_deletion_request: false,
            memorial: Some(false),
            moved_to_account_id: None,
        }
    }

    fn now() -> NaiveDateTime {
        NaiveDateTime::parse_from_str("2026-07-01 13:00:00", "%Y-%m-%d %H:%M:%S")
            .expect("fixed timestamp")
    }

    #[test]
    fn revocation_and_expiration_use_doorkeepers_exact_boundaries() {
        let mut token = candidate();
        token.revoked_at = Some(now());
        assert_eq!(
            authorize_candidate(&token, READ_ACCOUNTS, now()),
            Err(OAuthError::InvalidToken(InvalidTokenReason::Revoked))
        );
        token.revoked_at = Some(now() + TimeDelta::seconds(1));
        assert!(authorize_candidate(&token, READ_ACCOUNTS, now()).is_ok());

        let mut token = candidate();
        token.expires_in = Some(3_600);
        assert!(authorize_candidate(&token, READ_ACCOUNTS, now()).is_ok());
        assert_eq!(
            authorize_candidate(&token, READ_ACCOUNTS, now() + TimeDelta::milliseconds(1)),
            Err(OAuthError::InvalidToken(InvalidTokenReason::Expired))
        );
    }

    #[test]
    fn revocation_precedes_expiration_but_suspension_precedes_both() {
        let mut token = candidate();
        token.revoked_at = Some(now() - TimeDelta::seconds(1));
        token.expires_in = Some(1);
        assert_eq!(
            authorize_candidate(&token, READ_ACCOUNTS, now()),
            Err(OAuthError::InvalidToken(InvalidTokenReason::Revoked))
        );

        token.suspended_at = Some(now());
        assert_eq!(
            authorize_candidate(&token, READ_ACCOUNTS, now()),
            Err(OAuthError::UserDisabled)
        );
    }

    #[test]
    fn owner_state_is_checked_after_scope_and_application_only_tokens_remain_valid() {
        let mut token = candidate();
        token.disabled = Some(true);
        token.scopes = Some("push".to_owned());
        assert_eq!(
            authorize_candidate(&token, READ_ACCOUNTS, now()),
            Err(OAuthError::InsufficientScope(READ_ACCOUNTS))
        );

        let mut token = candidate();
        token.resource_owner_id = None;
        token.user_id = None;
        token.user_account_id = None;
        token.account_id = None;
        token.confirmed_at = None;
        token.approved = None;
        token.disabled = None;
        token.otp_required_for_login = None;
        token.role_requires_2fa = None;
        token.memorial = None;
        let authenticated = authorize_candidate(&token, READ_ACCOUNTS, now())
            .expect("application-only token remains an authenticated bearer");
        assert_eq!(authenticated.require_user(), Err(OAuthError::UserRequired));
        assert_eq!(authenticated.application_id(), Some(301));
    }

    #[test]
    fn nullable_applications_are_valid_but_dangling_application_ids_are_rejected() {
        let mut token_without_application = candidate();
        token_without_application.application_id = None;
        token_without_application.application_exists = false;
        let authenticated = authorize_candidate(&token_without_application, READ_ACCOUNTS, now())
            .expect("Mastodon permits tokens without an application");
        assert_eq!(authenticated.application_id(), None);

        let mut dangling_application = candidate();
        dangling_application.application_exists = false;
        assert_eq!(
            authorize_candidate(&dangling_application, READ_ACCOUNTS, now()),
            Err(OAuthError::InvalidToken(InvalidTokenReason::Unknown))
        );
    }

    #[test]
    fn exact_owner_state_produces_mastodon_failures() {
        let mut unconfirmed = candidate();
        unconfirmed.confirmed_at = None;
        assert_eq!(
            authorize_candidate(&unconfirmed, READ_ACCOUNTS, now())
                .expect("owner state does not invalidate an optional-auth token")
                .require_user(),
            Err(OAuthError::EmailUnconfirmed)
        );

        let mut pending = candidate();
        pending.approved = Some(false);
        assert_eq!(
            authorize_candidate(&pending, READ_ACCOUNTS, now())
                .expect("owner state does not invalidate an optional-auth token")
                .require_user(),
            Err(OAuthError::PendingApproval)
        );

        let mut disabled = candidate();
        disabled.disabled = Some(true);
        assert_eq!(
            authorize_candidate(&disabled, READ_ACCOUNTS, now())
                .expect("owner state does not invalidate an optional-auth token")
                .require_user(),
            Err(OAuthError::UserDisabled)
        );

        let mut memorial = candidate();
        memorial.memorial = Some(true);
        assert_eq!(
            authorize_candidate(&memorial, READ_ACCOUNTS, now())
                .expect("owner state does not invalidate an optional-auth token")
                .require_user(),
            Err(OAuthError::UserDisabled)
        );

        let mut moved = candidate();
        moved.moved_to_account_id = Some(2_002);
        assert_eq!(
            authorize_candidate(&moved, READ_ACCOUNTS, now())
                .expect("owner state does not invalidate an optional-auth token")
                .require_user(),
            Err(OAuthError::UserDisabled)
        );

        let mut missing_2fa = candidate();
        missing_2fa.otp_required_for_login = Some(false);
        assert_eq!(
            authorize_candidate(&missing_2fa, READ_ACCOUNTS, now())
                .expect("owner state does not invalidate an optional-auth token")
                .require_user(),
            Err(OAuthError::UserDisabled)
        );
    }
}
