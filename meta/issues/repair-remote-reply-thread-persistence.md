# Repair remote reply-thread persistence

## Summary

Following recovery of Pleroma Notes with nullable sensitivity, reply-thread job `472` retries with `remote reply thread persistence failed`. The child status exists but its parent remains unresolved. User approved investigating this downstream failure.

## Requirements

- Diagnose with read-only live metadata and source inspection; do not display credentials, private post bodies, or backups.
- Reproduce any defect on Secunda and implement a minimal independently reviewed repair.
- Preserve canonical identity, provenance, privacy, thread bounds and existing data. Do not bypass validation to force a parent link.

## Acceptance Criteria

- Establish the failing persistence boundary with evidence.
- Applicable regression tests, formatting and lint pass on Secunda.
- If deployed, verify the existing job completes and the parent/child relationship is correct, or record the remaining external blocker.

## Notes

- Related: [missing followed posts](diagnose-missing-followed-lain-com-posts.md).
- Live source at start: `f9b6af7`; recovered child `117252276514092511`.

## Bounded emoji writer repair (verified; deployment pending)

- Parent-provided live evidence: PostgreSQL `permission denied for table custom_emojis`
  at 11:55:57, 11:56:27, and 12:00:13 matches job 472's three attempts. The
  dedicated writer has SELECT/DELETE but no column INSERT/UPDATE or sequence USAGE.
  The remote parent actor was already imported; no profile/avatar repair is in scope.
- `upsert_remote_emojis` locks with `FOR UPDATE`, inserts remote metadata and queues
  emoji fetches; the cache worker updates only image metadata. Grant those columns
  and sequence USAGE, not table-wide INSERT/UPDATE or moderation UPDATE. Match
  preflight's allowlist and required privileges; leave runtime grants unchanged.
- All workloads use `/home/lain/rustodon-parity/reply-emoji-grants` on Secunda with
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
- TDD red: `red-worker.log` reports job 1 retained with attempts=1 and
  `remote reply thread persistence failed` before grants changed. The preceding
  transport setup attempt hit HTTPS against the plain HTTP fixture; using an HTTP
  test parent URI reached the intended persistence boundary.
- TDD red: `red-startup.log` reports old preflight accepting
  `REVOKE INSERT (shortcode) ON public.custom_emojis ...`. It ends 3 passed / 2
  failed: the expected negative-test failure, plus a later readiness refusal
  because that failed test's restore added a privilege the old policy rejected.
- Green, all on Secunda in `/home/lain/rustodon-parity/reply-emoji-grants`, with
  `CARGO_BUILD_JOBS=4` and `CARGO_TARGET_DIR` unset (task-local target):
  - Filtered worker harness: 1 passed, 62 filtered; worker lifecycle also passed
    (`green-worker.log`). Exact task-local harness generation/invocation:
    ```sh
    # Reference checkout revision verified read-only before this link was created.
    ln -s /home/lain/repos/rustodon/target/mastodon-v4.6.5 target/mastodon-v4.6.5
    sed 's/--test workers -- /--test workers reply_emoji_grants -- /' \
      tools/mastodon-fixture > tools/reply-emoji-fixture
    chmod +x tools/reply-emoji-fixture
    tools/reply-emoji-fixture worker-test > green-worker.log 2>&1
    ```
    The untracked task-local harness changes only Cargo's test filter, not fixture
    roles/grants/setup/teardown. The full ignored worker suite was not run.
  - `tools/mastodon-fixture startup-test`: 5 passed, including 29 emoji privilege
    mutations (17 missing required privileges and 12 over-grants), web/worker
    fail-closed checks, and healthy readiness (`green-startup.log`).
  - After removing only this task's reference symlink:
    `cargo fmt --all --check` (`green-fmt.log`);
    `cargo test --locked --all-targets --all-features`: 422 passed, 153 ignored
    across 29 test binaries (`green-tests.log`);
    `cargo clippy --locked --all-targets --all-features -- -D warnings`
    (`green-clippy.log`), all successful.
- Independent read-only review `ca8f3799-e92d-4602-bad4-3cd2d0815eb4`: no
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
verification of job 472 and the original child/parent relationship.

## Parent integration checkpoint — Secunda unavailable

- Main integrates emoji privileges (`0f32a3e`), raw-GIF REST rendering (`21f6b08`),
  shaped API collection reads (`230c26d`) and profile-media discovery/cache/refresh
  (`f1d8754`, `d3a3f85`, `2cb5659`). One module-declaration merge conflict retained
  both new worker test modules.
- Checksum-verified combined source was tested in `/home/lain/rustodon-parity/main`.
  Formatting, ordinary all-target/all-feature tests and strict Clippy passed.
  Logs: `/home/lain/rustodon-parity/combined-follow-repairs/{fmt,tests,clippy}.log`.
- Combined workers initially passed 70/71; actor deletion's old test inspected disk
  before draining the new durable Maintenance cleanup. Reviewed test-only correction
  `5b1437f` asserts exact retained cleanup paths, dispatches/acknowledges both jobs,
  and retains all physical-file assertions. Profile worktree full workers then
  passed 70/70; its separate readiness phase cannot certify combined grants.
- Required remaining combined gates after syncing the corrected test: formatting,
  Clippy, full worker fixture including lifecycle readiness, startup fixture, and
  the API HTTP fixture documented in the API issue. Ordinary tests passed before
  this test-only correction. Do not describe the combined deployment gate as green.
- Secunda SSH became unresponsive and `secunda.local` then failed DNS resolution;
  parent confirmed and asked the user to restore the host. No new image, live
  grant changes, cutover or recovery occurred for this repair set.
- Live applications still use `f9b6af7` image
  `da842de6c91857674c18051c2efdba8bc14297184ff324d91d9cbe1f5d99381a`.
  Job 472 exhausted four attempts and is dead-lettered with the same persistence
  error. Parent prepared a narrowly guarded replay, not executed.
- Locally retained grant-aware `redeploy-application.sh` accepts optional apply/
  inverse SQL files. Old preflight/backup precede stop; transactional grant apply
  precedes new preflight/start. If apply completion is uncertain, it does not race
  an inverse or restart either app: it retains the lock for manual backend/grant
  reconciliation. Confirmed-apply failures remove candidates, revert privileges,
  and restore original IDs before starting. Exact additive/inverse emoji scripts
  were independently reviewed; no profile grant delta is required.
- Eight Secunda mock safety cases passed in
  `/home/lain/rustodon-parity/grant-deploy-check/`: success, apply failure,
  surviving delayed apply after client failure, post-apply/pre-confirmation signal,
  candidate preflight failure, partial creation, start failure and inverse failure.
  These are mock evidence, not live rollback/restore tests.
