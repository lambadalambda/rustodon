# Test and document standalone Rustodon bootstrap

## Summary

Provide a bounded PostgreSQL-only integration lane and concise operator guide for
creating and validating a fresh Rustodon instance without Mastodon.

## Requirements

- Add a task-owned PostgreSQL 14 lane that executes the real installer and does
  not require Mastodon source, Rails, Sidekiq, Redis, or populated fixtures.
- Assert exact schema/migration fingerprints, baseline identities and roles,
  fresh distinct signing keys, empty content/media state, least-privilege grants,
  idempotent verification, and fail-closed partial/non-empty cases.
- Exercise first-admin login, media upload, public status creation, WebFinger,
  and ActivityPub in a bounded standalone smoke.
- Document prerequisites, secret handling, installation, startup, health checks,
  backup boundaries, and the separate existing-Mastodon cutover path.

## Acceptance Criteria

- The named standalone bootstrap lane passes from a clean checkout with only its
  documented PostgreSQL/container prerequisites.
- The guide produces a usable local-media instance without installing or running
  Mastodon.
- Existing ordinary and cutover checks remain unchanged and passing.
