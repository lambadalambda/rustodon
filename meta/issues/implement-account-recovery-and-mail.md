# Implement account recovery and mail

## Summary

Provide password reset, confirmation, and closed-instance administrator recovery.

## Requirements

- Deliver reset/confirmation mail through the durable mail lane when SMTP is
  configured and provide safe CLI fallback.
- Add CLI user creation and existing-user password/recovery reset.

## Acceptance Criteria

- Token expiry/reuse, SMTP failure, and CLI recovery tests preserve account
  security without implementing invites or open registration.

## Progress

- Added `rustodon admin reset-password --email ...` for existing users. The
  command reads the password from stdin when `--password` is omitted, hashes it
  with bcrypt, clears stale reset/sign-in tokens, removes browser sessions, and
  revokes the user's OAuth access tokens and pending authorization grants
  transactionally.
- CLI help coverage proves both recovery commands are exposed. Password-reset
  request/edit/update pages enforce CSRF, six-hour expiry, single-use
  consumption, password length limits, and session/token/grant invalidation.
  New SMTP-backed tokens use Devise's PBKDF2-derived, column-specific HMAC
  digests with a legacy SHA-256 fallback for existing Rust-owned fixture rows.
- Added durable AES-GCM-encrypted reset and confirmation jobs in the Mail lane,
  SMTP TLS/authentication configuration, retryable transport failures, and
  atomic token/outbox writes. Confirmation links are handled by
  `/auth/confirmation` and expire after two days.
- Added `rustodon admin create-user --email ... --username ...`, stdin password
  fallback, local validation, bcrypt credentials, RSA signing keys, initial
  `account_stats`, and SMTP-gated confirmation delivery. SMTP-disabled
  instances create confirmed local users as the safe administrative fallback.
- Recovery rejects external-auth users, removes their push subscriptions during
  reset, and applies local-process IP/email reset throttles matching Mastodon's
  25-per-five-minute IP and 5-per-thirty-minute email boundaries.
- Local mail/config/CLI tests pass, and the guarded recovery case checks pending
  OAuth-grant revocation. The restored PostgreSQL fixture now passes the full
  14-case differential suite, including a real `admin create-user` and
  `admin reset-password` run, encrypted confirmation outbox inspection,
  confirmation consumption/replay rejection, and confirmation expiry.
- Acceptance is complete: the local SMTP protocol integration verifies worker
  delivery and decrypted reset-link contents, while the refused-connection
  test verifies retry classification. Cross-process throttling remains a
  deployment-level follow-up; streaming kill-event publication is covered by
  the operational fixture integration.
- Browser password-reset request/update failures and invalid confirmation links
  now negotiate escaped HTML documents for browser requests while retaining
  JSON error envelopes for non-HTML clients; the pure response contract is
  covered by web tests and the password-recovery differential flow remains
  green.
