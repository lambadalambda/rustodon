# Repair remote reply-thread persistence

## Summary

Following recovery of the affected Pleroma Notes, a reply-thread job continued
retrying with `remote reply thread persistence failed`. The child status existed
but its parent remained unresolved. User approved investigating this downstream
failure.

## Requirements

- Diagnose with read-only live metadata and source inspection; do not display credentials, private post bodies, or backups.
- Reproduce any defect on an isolated worker and implement a minimal independently reviewed repair.
- Preserve canonical identity, provenance, privacy, thread bounds and existing data. Do not bypass validation to force a parent link.

## Acceptance Criteria

- Establish the failing persistence boundary with evidence.
- Applicable regression tests, formatting and lint pass on an isolated worker.
- If deployed, verify the existing job completes and the parent/child relationship is correct, or record the remaining external blocker.

## Notes

- Related: [missing followed posts](diagnose-missing-followed-lain-com-posts.md).
- Live source at start: `f9b6af7`; the affected child status was recorded in the
  external diagnostic evidence.

## Bounded emoji writer repair (verified; deployment pending)

- Parent-provided live PostgreSQL evidence showed repeated
  `permission denied for table custom_emojis` errors matching the affected job's
  three attempts. The dedicated writer has SELECT/DELETE but no column
  INSERT/UPDATE or sequence USAGE.
  The remote parent actor was already imported; no profile/avatar repair is in scope.
- `upsert_remote_emojis` locks with `FOR UPDATE`, inserts remote metadata and queues
  emoji fetches; the cache worker updates only image metadata. Grant those columns
  and sequence USAGE, not table-wide INSERT/UPDATE or moderation UPDATE. Match
  preflight's allowlist and required privileges; leave runtime grants unchanged.
- All workloads use a task-owned workspace on an isolated worker with
  its own target directory. No production role changes or live-instance access.

### Implementation and verification

- Added eight INSERT columns and eight UPDATE columns for `custom_emojis`, plus
  USAGE only on `custom_emojis_id_seq`. Preflight requires every new privilege and
  permits exactly those additions. Moderation/category/shortcode/domain UPDATE,
  broad table INSERT/UPDATE, sequence SELECT/UPDATE, and grant options stay fenced.
  Runtime privileges are unchanged.
- New `tests/workers/reply_emoji_grants.rs` uses the actual dedicated writer, not
  the fixture owner, through the fetched-parent thread handler. It covers generated
  emoji IDs, existing metadata refresh without moderation/creation-time changes,
  durable job completion, parent/child/conversation/reply-count persistence,
  exact emoji outbox payloads, and both original/static cache installations.
  The owner connection is fixture setup/cleanup only. A two-line feature-gated
  thread-fetcher endpoint hook reuses the existing debug-only fixture transport;
  ordinary release transport and profile/avatar handling are unchanged.
- A historical external TDD RED reports job 1 retained with attempts=1 and
  `remote reply thread persistence failed` before grants changed. The preceding
  transport setup attempt hit HTTPS against the plain HTTP fixture; using an HTTP
  test parent URI reached the intended persistence boundary.
- A historical external TDD RED reports old preflight accepting
  `REVOKE INSERT (shortcode) ON public.custom_emojis ...`. It ends 3 passed / 2
  failed: the expected negative-test failure, plus a later readiness refusal
  because that failed test's restore added a privilege the old policy rejected.
- Green, all on an isolated worker in a task-owned workspace, with
  `CARGO_BUILD_JOBS=4` and `CARGO_TARGET_DIR` unset (task-local target):
  - The filtered worker harness and worker lifecycle passed: 1 selected, 62
    filtered. A temporary filtered invocation changed only Cargo's test filter,
    not fixture roles, grants, setup, or teardown. The full ignored worker suite
    was not run.
  - `tools/mastodon-fixture startup-test`: 5 passed, including 29 emoji privilege
    mutations (17 missing required privileges and 12 over-grants), web/worker
    fail-closed checks, and healthy readiness.
  - After removing only this task's reference symlink:
    `cargo fmt --all --check`;
    `cargo test --locked --all-targets --all-features`: 422 passed, 153 ignored
    across 29 test binaries;
    `cargo clippy --locked --all-targets --all-features -- -D warnings`;
    all succeeded.
- Independent read-only review: no
  correctness, least-privilege, or architecture blocker. Its runtime sequence
  coverage nit was addressed by checking both USAGE and UPDATE. Failed-test
  panic cleanup remains a nonblocking fixture-hygiene observation; no unrelated
  cleanup refactor was added.

### Minimal deployment delta (not applied here)

The parent owns backup/stop, application of this transaction, candidate preflight,
start/cutover, and replay. The old binary rejects these additional grants; the new
binary requires them. Do not use a broad grant refresh or resume the old binary
until applying the inverse. Supply the actual dedicated writer as `writer_role`;
never substitute the runtime role. These statements assume the verified previous
state (no emoji INSERT/UPDATE column grants and no sequence USAGE).

```sql
\set ON_ERROR_STOP on
BEGIN;
GRANT INSERT (
  shortcode, domain, uri, image_remote_url, disabled, visible_in_picker,
  created_at, updated_at
) ON TABLE public.custom_emojis TO :"writer_role";
GRANT UPDATE (
  uri, image_remote_url, updated_at, image_content_type, image_file_name,
  image_file_size, image_storage_schema_version, image_updated_at
) ON TABLE public.custom_emojis TO :"writer_role";
GRANT USAGE ON SEQUENCE public.custom_emojis_id_seq TO :"writer_role";
COMMIT;
```

Inverse transaction for rollback to the previous grant contract:

```sql
\set ON_ERROR_STOP on
BEGIN;
REVOKE INSERT (
  shortcode, domain, uri, image_remote_url, disabled, visible_in_picker,
  created_at, updated_at
) ON TABLE public.custom_emojis FROM :"writer_role";
REVOKE UPDATE (
  uri, image_remote_url, updated_at, image_content_type, image_file_name,
  image_file_size, image_storage_schema_version, image_updated_at
) ON TABLE public.custom_emojis FROM :"writer_role";
REVOKE USAGE ON SEQUENCE public.custom_emojis_id_seq FROM :"writer_role";
COMMIT;
```

No live-instance files, credentials, production roles, or sibling target were
accessed or changed. Profile-media sibling work remains separate; this repair
adds no account grants. Issue remains open for parent-managed deployment/replay
verification of the affected job and original child/parent relationship.

## Parent integration checkpoint — isolated worker unavailable

- Main integrates emoji privileges (`0f32a3e`), raw-GIF REST rendering (`21f6b08`),
  shaped API collection reads (`230c26d`) and profile-media discovery/cache/refresh
  (`f1d8754`, `d3a3f85`, `2cb5659`). One module-declaration merge conflict retained
  both new worker test modules.
- Checksum-verified combined source was tested in a task-owned workspace.
  Formatting, ordinary all-target/all-feature tests and strict Clippy passed.
  Historical external run artifacts are not in the repository.
- Combined workers initially passed 70/71; actor deletion's old test inspected disk
  before draining the new durable Maintenance cleanup. Reviewed test-only correction
  `5b1437f` asserts exact retained cleanup paths, dispatches/acknowledges both jobs,
  and retains all physical-file assertions. Profile worktree full workers then
  passed 70/70; its separate readiness phase cannot certify combined grants.
- Required remaining combined gates after syncing the corrected test: formatting,
  Clippy, full worker fixture including lifecycle readiness, startup fixture, and
  the API HTTP fixture documented in the API issue. Ordinary tests passed before
  this test-only correction. Do not describe the combined deployment gate as green.
- Isolated execution became unavailable after timeouts and DNS resolution failure.
  No new image, live grant changes, cutover, or recovery occurred for this repair
  set during that interruption.
- Live applications still used source `f9b6af7`. The affected job exhausted four
  attempts and remained dead-lettered with the same persistence error; a narrowly
  guarded replay was prepared but not executed.
- An independently reviewed external deployment process preserved the exact
  reversible grant delta, and its eight-case mock safety suite passed. No profile
  grant delta was required. This is mock evidence, not a live rollback or restore
  test; unpublished helper details are not in the repository.

## Isolated validation environment

- Remaining workloads moved to a bounded isolated environment after the earlier
  one became unstable; unrelated resources and default container connections were
  not altered.
- The replacement environment was x86-64 with Rust 1.97.1 cached. Validation ran
  in a bounded tool container before cross-building the ARM64 release. Source
  transfer excluded Git metadata, build targets, environment files, instance state,
  and backups. No replacement Mastodon source was fetched; worker, startup, and API
  fixtures used committed data and pinned container images.

## Combined isolated worker validation and live recovery — 2026-09-11 UTC

- A task-owned workspace validated checksum-matched source
  `b2937cf905c8805371651d3b06cae5f73473f21e` with Rust 1.97.1, a four-CPU
  quota, 8 GiB memory, and three Cargo jobs.
- Formatting, all-target/all-feature tests (**430 passed, 162 ignored**), strict
  Clippy, full workers (**71 passed**, including lifecycle readiness), startup
  (**5 passed**) and the focused API HTTP fixture (**1 passed**) all passed.
  Historical external run artifacts are not in the repository.
- Isolated-environment corrections, not source fixes, removed capability-based
  bypass of a directory-permission regression and allowed synchronized mode-600
  inputs to be read. Six worker tests also required the three media files from
  the pinned Mastodon fixtures; only those files were extracted from the pinned
  harness image and their hashes were checked. No reference checkout was fetched,
  replaced, or claimed present; pinned-source contract tests were previously
  verified in an earlier external run, not rerun in this validation.
- The AArch64 release cross-build used explicit Rust 1.97.1 and
  `--locked --release --no-default-features --bin rustodon --target aarch64-unknown-linux-gnu`.
  Packaging used the same pinned ARM64 Debian base and distribution CA bundle as
  the previous deployment. Independent review prompted fresh-only staging and
  explicit native loader checks.
- The Linux/ARM64 candidate carried the full source revision label. Native loader
  and CLI checks passed, the transferred package identity matched, and all
  **6062** packaged runtime inputs passed hash verification. The bundled frontend
  subsequently loaded in the browser.
- The reviewed grant-aware application-only deployment completed on
  **2026-09-11**: old preflight, consistent backup, both apps stopped, exact emoji
  grants committed, new preflight, app replacement, readiness, and identity
  checks. Evidence is a historical external artifact not in the repository. The
  backup was not restore-tested. PostgreSQL and Redis services, volumes, local
  actor identity, and the accepted follow were preserved; the prior application
  pair remains stopped and available for rollback.
- **Rollback warning:** the old binary rejects the new emoji grants.
  Application-only rollback must apply the reviewed exact inverse before
  restoring the old pair; never run either old app alongside the current pair.
  An uncertain grant apply must be reconciled before any inverse/restart. This
  rollback was not needed.
- A bounded replay preserved the affected job's payload, attempts, and lease
  generation. It completed and removed the job row; the child now references the
  correct Pleroma parent, account, canonical object, and public ancestor context.
- Post-recovery worker/local/public readiness passed with **zero queued jobs and
  zero dead letters**. No credentials, private post bodies or backup contents were
  displayed. Thread-repair acceptance is satisfied; issue archived. Expanded
  real-peer federation matrices remain separate and unexecuted.
