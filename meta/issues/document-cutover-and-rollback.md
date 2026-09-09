# Document cutover and rollback

## Summary

Make the supported maintenance-window migration operationally repeatable.

## Requirements

- Document snapshot, Sidekiq drain, preflight, process/Nginx startup order,
  smoke tests, Redis removal, rollback triggers, and Mastodon restart.
- Add a non-destructive smoke command covering auth, media, reads/writes,
  workers, and federation.

## Acceptance Criteria

- A rehearsed fixture cutover and rollback requires no Mastodon-table migration reversal.

## Progress

- Added [`docs/cutover.md`](../../docs/cutover.md) with snapshot, Sidekiq drain,
  preflight, isolated operational-schema migration, startup ordering, proxy
  handoff, smoke checks, rollback triggers, and Mastodon restart steps.
- Added `tools/rustodon-smoke`, a non-destructive operator command that checks
  health/readiness, authenticated account/status/marker reads, an empty marker
  write, one local Paperclip media URL, WebFinger, and ActivityPub actor
  discovery. The bearer token is supplied through `RUSTODON_SMOKE_TOKEN`.
- The runbook and command cover the core sequence. The complete writer ACL
  recipe is now executable from `docs/mastodon-writer-grants.sql`.
- Added `mise run cutover-integration`, which runs the pinned fixture through
  operational-schema migration, runtime/writer preflight, Rustodon worker/web
  startup, smoke checks, Rustodon shutdown, Mastodon web reopen, Rails
  verification, and public catalog/schema/data/media preservation checks.
- The smoke client uses a temporary mode-restricted curl header file for the
  bearer token, supports the fixture's configured host header, and uses curl's
  native HEAD mode for media checks.

## Open Follow-up

- The writer-enabled path now has an explicit least-privilege ACL recipe in
  `docs/mastodon-writer-grants.sql`; the fixture executes that same script for
  writer-enabled integration coverage. Live production cutover, Nginx handoff,
  and worker/streaming rehearsal remain release-level follow-up. See
  [`review-writer-acl-cutover.md`](review-writer-acl-cutover.md).

## Resolution

- `mise run cutover-integration` passes for Mastodon v4.6.5. The fixture
  returns from Rustodon to a freshly started pinned Mastodon web process and
  Rails verification without reversing any Mastodon-table migration.
- Public catalog, schema, stable data, stable authentication data, and the
  complete Paperclip media tree compare unchanged. Expected authentication
  tracking timestamps/IPs are excluded from the stable-data comparison.
