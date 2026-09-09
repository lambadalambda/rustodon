# Implement browser authentication

## Summary

Authenticate existing users with passwords, TOTP, backup codes, and Rustodon sessions.

## Requirements

- Verify existing bcrypt/TOTP data, atomically consume backup codes, enforce
  account state and role-required 2FA, and implement secure sessions/CSRF.
- Add login/logout/session expiry and durable abuse limits.

## Acceptance Criteria

- Differential vectors cover valid and invalid password/2FA flows without
  requiring Rails cookies or WebAuthn.

## Progress

- Added Rails-compatible bcrypt password verification, six-digit SHA-1 TOTP
  verification with one-step drift and consumed-timestep replay protection,
  and constant-time matching for plaintext fixture and bcrypt backup codes.
- Added transactional browser login persistence. Password, account-state,
  role-required 2FA, invalid-factor, and durable hourly 2FA-attempt failures
  are recorded in `login_activities`; successful TOTP and backup-code paths
  update the corresponding user state atomically.
- Added `POST /auth/sign_in`, `GET /auth/session`, and CSRF-protected
  `POST`/`DELETE /auth/sign_out` with expiring `session_activations`, secure
  HttpOnly/SameSite session cookies, double-submit CSRF cookies, and session
  touch/delete behavior.
- Added guarded differential coverage for invalid password, missing and
  invalid 2FA, backup-code non-consumption, successful Rust backup-code
  consumption, and session-row lifecycle. The browser authentication case
  passes against the pinned Mastodon 4.6.5 fixture.
- The login page emits a double-submit CSRF token, and sign-out and OAuth
  consent POSTs enforce it. Sign-in POST validation against Rails'
  authenticity-token flow is covered by a dedicated unit vector and the
  guarded browser case supplies the matching token.
- WebAuthn challenge verification, full HTML login/2FA forms, mail/recovery
  workflows, and distributed abuse controls are separate follow-up issues or
  explicitly outside the v1 scope.
- The acceptance criterion is complete: guarded vectors cover invalid password,
  missing/invalid 2FA, successful backup-code authentication, backup-code
  consumption, and Rustodon session create/touch/delete lifecycle.
- Browser session activations now also create and own Mastodon-compatible
  `read write follow` OAuth tokens, use the optional `superapp` application,
  and delete the owned token during logout. The guarded browser case verifies
  the linkage and cleanup.
- Browser form failures now negotiate HTML for `text/html` requests while
  retaining Mastodon-compatible JSON envelopes for API clients. Login errors
  preserve the submitted email and issue a fresh CSRF cookie when needed;
  password-reset and confirmation failures render escaped, actionable forms or
  links. The guarded browser case verifies HTML content types for invalid
  password and 2FA submissions.
- Browser settings now support encrypted TOTP setup, recovery-code regeneration,
  password-challenged disable, and required-role/WebAuthn cleanup behavior. The
  guarded management case verifies ciphertext at rest, bcrypt recovery codes,
  role-protected disable requests, and state restoration against Mastodon 4.6.5.

## Follow-up Gap

- The pinned Rails reference separates authentication eligibility from
  functional access: `User#active_for_authentication?` rejects only memorial
  accounts, while `ApplicationController#require_functional!` gates ordinary
  controllers after sign-in. Rustodon currently rejects disabled, suspended,
  and moved accounts during password authentication and excludes them from
  persisted browser sessions. Align these layers so non-memorial users can
  authenticate and reach the appropriate post-login functional guard without
  weakening API authorization.

- Rustodon now follows that separation for the implemented browser session
  boundary: disabled, suspended, and moved non-memorial accounts authenticate
  and retain usable browser sessions, while memorial accounts remain rejected.
  The differential browser-authentication case covers all four lifecycle
  states. Functional API authorization remains governed by the existing OAuth
  lifecycle policy; ordinary web functional guards and full Rails recovery
  flows remain separate acceptance work.

- Browser sessions now expose their functional state to the web boundary, and
  OAuth consent rejects non-functional sessions before creating an
  authorization grant. The lifecycle differential case covers this guard for
  disabled, suspended, and moved accounts.
