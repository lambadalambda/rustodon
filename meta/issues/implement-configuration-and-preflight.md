# Implement configuration loading and preflight checks

## Summary

Load the minimal Mastodon-compatible environment needed by Rustodon and reject
unsafe or unsupported cutovers before serving traffic.

## Requirements

- Parse `LOCAL_DOMAIN`, optional `WEB_DOMAIN`, alternate domains, PostgreSQL
  connection settings, local Paperclip root settings, trusted proxies, SMTP,
  and required cryptographic secrets.
- Keep secret values out of logs, debug output, and error messages.
- Implement a `rustodon preflight` command with machine-readable failure status
  and useful operator diagnostics.
- Verify PostgreSQL connectivity, supported schema version, required
  `timestamp_id()` function and sequences, canonical domains, local account
  key availability, and media-root readability/writability.
- Compare v1-critical table columns, types, nullability, defaults, constraints,
  and indexes against the pinned physical-schema fingerprint instead of
  trusting `schema_migrations` alone.
- Decrypt or load every local signing key used by the target schema, derive its
  public key, and complete an in-memory sign/verify check against the stored
  public key without changing key material.
- Detect and reject object storage, unknown schema versions, unreadable local
  keys, and unsupported enabled SSO providers.
- Detect pending scheduled statuses, active local polls, pending account
  deletions, WebAuthn-only users, active relays, configured automated status
  cleanup policies, and non-empty Mastodon Sidekiq queues.
- Distinguish fatal incompatibilities from warnings that do not affect data
  safety.

## Acceptance Criteria

- Unit tests cover defaults, invalid values, secret redaction, and domain
  normalization.
- Integration tests cover supported and unsupported database schemas.
- Integration tests reject each v1-critical physical-schema mismatch even when
  `schema_migrations` claims the supported version.
- Preflight exits successfully for the canonical 4.6.5 fixture.
- Each explicitly listed unsupported configuration or active-data condition
  produces a targeted failure with a remediation hint.
- A missing, corrupt, mismatched, or undecryptable local signing key fails
  before Rustodon serves traffic.
- Running preflight does not mutate Mastodon-owned data or media.

## Notes

- Depends on `map-mastodon-4-6-5-schema.md` and
  `pin-mastodon-4-6-5-fixtures.md`.
- Sidekiq queue drain validation may require an optional Redis connection only
  during cutover; Redis is not a Rustodon runtime dependency.
