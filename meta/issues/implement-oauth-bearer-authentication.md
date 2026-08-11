# Implement existing OAuth bearer-token authentication

## Summary

Authenticate requests using OAuth applications and access tokens already
stored by Mastodon 4.6.5 without issuing new tokens or implementing browser
login yet.

## Requirements

- Accept standard `Authorization: Bearer` tokens stored in
  `oauth_access_tokens`.
- Validate revocation, expiry, resource owner, application, and user/account
  functional state.
- Enforce Mastodon's broad and granular scope relationships for the first read
  APIs.
- Represent unauthenticated, invalid-token, wrong-scope, and disabled-user
  failures consistently at the HTTP layer.
- Do not update token last-used metadata in this read-only milestone. Record the
  omission for the later authenticated-write phase.
- Never log bearer tokens or include them in tracing attributes.
- Keep authorization-code issuance, dynamic application registration, browser
  sessions, and TOTP login out of this issue.

## Acceptance Criteria

- Existing valid fixture tokens authenticate without modification.
- Tests cover revoked, expired, unknown, application-only, disabled-user, and
  insufficient-scope tokens.
- Broad `read` and relevant granular read scopes authorize the expected
  endpoints.
- Differential request cases match Mastodon for the supported authentication
  outcomes.
- Logs and errors contain no full or partial fixture token value.
- Authentication leaves the fixture database byte-for-byte logically
  unchanged at the row level.

## Notes

- Depends on `map-mastodon-4-6-5-schema.md` and
  `build-differential-test-harness.md`.
- This is deliberately the first authentication slice because it unlocks
  existing mobile clients without implementing all of Devise and Doorkeeper.
