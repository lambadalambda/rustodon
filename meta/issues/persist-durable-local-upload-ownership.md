# Persist durable local upload ownership

## Current status — complete (2026-09-18)

Independent persistence review `688362` approved; committed in `949d503`.
Subsequent worker/HTTP integration supplies committed replay retention and
successful raw-only retirement coverage while preserving published outputs.
Fresh/upgrade/privilege evidence below and the integration dependencies are complete.

Parent explicitly approves closure and archival. This status supersedes earlier
“open”, “uncommitted”, “review pending” and closure-proposal instructions below;
those record historical handoff stages, not outstanding work. No gates were rerun
for this docs-only reconciliation. Actual transport-loss/power-loss simulation,
full fixture/release matrices and remote-media implementation remain unclaimed.
The [frontend rich-media parent](support-frontend-video-attachments.md) is now
**complete** on combined reviewed local and remote evidence after `fe6e461`.

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

## Implementation and review (2026-09-18)

- Migration 5 adds writer-only `rustodon.local_uploads`; the exact PostgreSQL 14.23
  catalog fingerprint, writer grants/preflight, known-writer upgrade grants, and
  operational fixture downgrade prefix include it. Standalone bootstrap inherits
  it through the existing migration and writer-grant paths. No public DDL changes.
- Transaction primitives live in `mastodon::local_uploads`: `stage_in`, `load_in`,
  `accept_in`, `claim_in`, `register_outputs_in`, `publish_in`,
  `discard_staging_in`, `abandon_in`, and `forget_orphan_in`. No live callers.
- Staging registers bounded raw MIME/size/SHA-256 and a derived raw path before
  write, without a legacy rollback job. Acceptance and a generation-keyed outbox
  intent commit together. Claims fence stale publication; the exact output
  manifest is immutable across retries. Ready publication preserves current
  description/focus and is retry-idempotent. Deletion retains cleanup ownership.
- Integration MUST hold the existing account lock around filesystem writes and
  related transaction boundaries: SQL fencing does not fence an open file writer.
  Caller authorization/account-lifecycle checks remain mandatory. Reconcile an
  ambiguous commit by loading the exact identity, never by assuming rollback.
- Follow-up: register the processing job on an existing lane before wiring
  acceptance; implement private raw storage/hash verification, bounded processor,
  recovery scan, raw-only cleanup/retirement after readiness (preserving published
  outputs), orphan cleanup, and HTTP pending/edit/delete behavior. The old
  synchronous staging/publish/cleanup bodies are unchanged. No worker behavior,
  new queue lane, media filesystem changes, or production operations here.
- Evidence: initial missing-table assertion red on the pinned restored Linux PG
  fixture; focused upload tests 3/3 green; operational fresh/idempotency/drift test
  green; v4-to-v5 known-writer upgrade and restricted role checks green; existing
  standalone bootstrap exact test green, including its synchronous HTTP smoke.
  This does not claim the complete container fixture launcher or codec/browser/
  peer gates ran. See DEVLOG for commands and lint boundaries.
- Independent correctness/architecture review found no blocker/high findings for
  this persistence-only slice. Follow-up worker tests must establish committed
  replay/ambiguous-commit and competing-attempt filesystem behavior; sequential
  transaction tests here do not claim those guarantees end to end.
- Keep this subissue open through integration until successful raw cleanup can
  retire ownership without treating live published output as orphaned. The
  persistence foundation is safe to commit separately; local uploads are not
  enabled by it.
