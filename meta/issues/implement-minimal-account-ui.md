# Implement the minimal account UI

## Summary

Provide Rust-owned authentication and essential account settings pages.

## Requirements

- Implement login, TOTP/backup challenge, logout, reset/confirmation, profile,
  avatar/header, posting preferences, and basic security pages.
- Preserve session/CSRF behavior and accessible form operation.

## Acceptance Criteria

- Browser tests cover every supported flow without recreating the full Rails
  settings or admin interface.

## Progress

- Added authenticated `/settings/delete` and `/settings/delete/` pages with
  CSRF-protected password or username confirmation, immediate sign-out, and a
  durable local deletion request. Full account-content purge remains outside
  the current UI milestone.
- The browser account-settings differential case now covers profile, preference,
  security, deletion-page, and multipart avatar/header upload flows. It proves
  that profile saves preserve existing Paperclip files when no media is sent,
  and that browser uploads persist both profile images and descriptions before
  restoring the database and media baseline.
- The guarded browser 2FA management case now covers encrypted TOTP setup,
  bcrypt recovery-code generation and regeneration, password-challenged
  disable, required-role disable protection, WebAuthn cleanup, and complete
  database restoration against the pinned Mastodon 4.6.5 fixture. Remaining
  client/mobile acceptance evidence remains open.
- Settings pages now render an accessible CSRF-protected HTML logout form, while
  `POST`/`DELETE /auth/sign_out` preserve the pinned JSON `redirect_to` contract
  and revoke the browser session. The browser authentication and account-settings
  differential cases both exercise logout and reject the old session afterward.
