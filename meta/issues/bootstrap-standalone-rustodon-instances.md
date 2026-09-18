# Bootstrap standalone Rustodon instances

## Summary

Provide an easy supported path to create a new Rustodon instance from an empty
PostgreSQL database without installing or running Mastodon. The bootstrap should
also give tests a fast, production-shaped database baseline that does not depend
on starting Rails.

## Requirements

- Create the exact Mastodon-compatible PostgreSQL schema Rustodon supports from
  a versioned, reviewable repository artifact rather than a live Mastodon
  checkout or runtime.
- Provision Rustodon operational state, least-privilege runtime and writer
  access, required database functions, and an empty local media root.
- Initialize a new instance identity and first local administrator without
  copying fixture users, domains, keys, tokens, or media.
- Keep secrets operator-supplied or freshly generated and never write them to
  logs or tracked files.
- Fail closed on non-empty, partially initialized, drifted, or unsupported
  databases; make safe completed steps idempotent where practical.
- Preserve the existing in-place Mastodon cutover path.
- Document a small-instance setup flow and expose a bounded disposable test
  lane that uses the same bootstrap contract.

## Acceptance Criteria

- A documented command starts from a fresh PostgreSQL 14 database and empty
  media directory, without Mastodon source, images, Rails, or Sidekiq, and
  produces a Rustodon instance that passes preflight and startup validation.
- The first administrator can log in, create a public status with media, and be
  fetched through WebFinger and ActivityPub.
- A fresh bootstrap is covered by an isolated automated test that verifies the
  supported schema fingerprint and contains no fixture identities or content.
- Existing restored-Mastodon cutover and ordinary checks continue to pass.
- Independent blocker/high review passes.

## Notes

- Do not use the populated compatibility fixture as an installer.
- Keep this prototype-scale: prefer one PostgreSQL/local-media deployment path
  over a general orchestration framework.
- Tracked subissues:
  - [Package a standalone Mastodon-compatible schema](package-standalone-mastodon-schema.md)
  - [Implement the standalone instance bootstrap command](implement-standalone-bootstrap-command.md)
  - [Test and document standalone Rustodon bootstrap](test-and-document-standalone-bootstrap.md)
