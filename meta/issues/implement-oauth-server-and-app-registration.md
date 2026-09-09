# Implement the OAuth server and app registration

## Summary

Support mobile-client registration, authorization, issuance, and revocation.

## Requirements

- Implement `POST /api/v1/apps`, app credential verification, authorization
  code with PKCE, required refresh behavior, token issuance, and revocation.
- Preserve existing clients/tokens and Mastodon scope semantics.

## Acceptance Criteria

- The recorded mobile client authorizes through both existing and newly
  registered applications with Mastodon-compatible state transitions.

## Progress

- Added public `POST /api/v1/apps` registration with both slash forms. It
  persists Doorkeeper-compatible application credentials, defaults omitted
  scopes to `read`, normalizes array redirect URIs, returns the credential
  application serializer shape including VAPID metadata, enforces the pinned
  name/redirect/website limits, configured scopes, and redirect/website URL
  rules. Guarded differential coverage compares the response fields while
  masking generated IDs and secrets and removes only the newly created
  application rows so existing OAuth tokens remain intact.
- Added authenticated `GET /api/v1/apps/verify_credentials` in both slash
  forms. It authenticates application-backed bearer tokens without requiring a
  read scope, returns only the public application serializer fields, and
  preserves Mastodon's invalid, revoked, expired, and application-only token
  behavior.
- App registration now matches Mastodon's observed scope edge cases: array-form
  scopes fall back to `read`, while scalar scopes are deduplicated in their
  original order before persistence.
- Added `POST /oauth/token` for the configured `client_credentials` grant. It
  supports client-secret POST and Basic authentication, application-scoped
  default/explicit scopes, active-token reuse, application-only token
  persistence, and Mastodon-compatible success/error headers and envelopes.
- Added `POST /oauth/revoke` with client authentication, access/refresh token
  lookup hints, ownership checks, idempotent unknown-token handling, and
  persisted revocation timestamps.
- Added browser-session authorization consent at `GET`/`POST /oauth/authorize`,
  one-time `oauth_access_grants` with S256 PKCE validation, redirect/state
  preservation, denial redirects, and `authorization_code` token exchange.
  Guarded differential coverage proves grant revocation, token issuance, PKCE,
  replay rejection, and generated-token response compatibility.
- Added `GET`/`POST /oauth/userinfo` with profile-scope authorization, OAuth user
  claims, private caching, and authorization variance. Added
  `GET /.well-known/oauth-authorization-server` with Mastodon's endpoint,
  scope, grant, PKCE, and cache metadata. Guarded differential coverage now
  compares both endpoints against Mastodon 4.6.5.
  Refresh is disabled in the pinned Mastodon configuration;
  Rustodon should keep it disabled unless compatibility requirements change.
 - Registration throttling uses the shared PostgreSQL rate-limit window when an
   operational pool is configured, with a bounded local fallback. Absent VAPID
   configuration serializes as JSON `null`, matching Doorkeeper's application
   serializer. Doorkeeper's `access_token` and `bearer_token`
   parameter-based bearer transports now work for query and form parameters;
   explicit `Authorization` headers remain authoritative.
 - Unauthenticated OAuth authorization now preserves the validated local
   authorization request through browser sign-in, returning to the consent
   page instead of unconditionally redirecting to `/` after login.
