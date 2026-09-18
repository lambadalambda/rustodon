# Persist durable local upload ownership

## Summary

First bounded part of [local rich-media uploads](support-local-rich-media-uploads.md).
Current filename-null staging is rollback-only and cannot safely outlive the HTTP
account lock. Add explicit durable ownership without changing Mastodon tables.

## Requirements

- Introduce a minimal Rust-owned upload-state record for media/account identity,
  staging/accepted lifecycle, generation, raw input identity, and exact cleanup
  responsibility. Prefer existing processing fields/jobs over duplicated state.
- Preserve least-privilege reader/writer/worker boundaries and operational schema
  migration, validation, upgrade, and standalone bootstrap contracts.
- Define transaction-level staging/acceptance/terminalization and recovery
  invariants before exposing any new upload behavior.
- No new queue lane, public schema changes, production migration, or remote-media
  integration in this subissue.

## Acceptance Criteria

- Focused database tests reproduce missing durable ownership then pass for the
  new contract, including interrupted staging, duplicate/stale generations,
  deletion, and artifact-manifest validation as applicable to this layer.
- Fresh schema and upgrade validation and least-privilege access tests pass in
  disposable task-owned databases.
- Independent review approves correctness and compactness; parent upload issue
  remains open until HTTP/worker/browser integration is verified.

## Scope checkpoint

- 2026-09-18: local upload inspection establishes new durable state is necessary:
  current cleanup deletes filename-null staging, filename-present staging is
  spared indefinitely, and publication is not retry-idempotent. Implement this
  persistence boundary separately before worker/API integration. A Rust-owned
  table is preferred to hiding internal state in public media metadata.
