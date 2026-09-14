# Diagnose missing remote avatar and banner

## Summary

The user reports that followed remote account `lain@lain.com` lacks its avatar and banner on the local instance.

## Requirements

- Distinguish remote actor discovery/profile metadata problems from media download, persistence or rendering failures.
- Use read-only live diagnostics first; do not display credentials or private content.
- Reproduce any defect on an isolated worker before a minimal independently reviewed fix; preserve existing remote identity and moderation settings.

## Acceptance Criteria

- Identify the failing boundary for both avatar and banner.
- Applicable regression checks pass for any repair.
- Verify profile metadata and rendered media after recovery, or record the remaining external blocker.

## Notes

- Reported while investigating [reply-thread persistence](repair-remote-reply-thread-persistence.md); keep the repairs topical.

## Bounded repair in progress

- Discovery drops `icon`/`image`; verified Update stores only remote URLs, while REST
  intentionally requires cached Paperclip metadata. No hotlink fallback is appropriate.
- Implement presence-aware shared image persistence and bounded profile media jobs,
  fencing actor identity, URL, generation and moderation; explicit null removes caches.
- Add a DB-canonical, signed operator refresh for an existing remote account. No live
  origins, environment files or instance workloads are used in implementation tests.
- Work used a task-specific worktree; the sibling task owned writer grants and preflight.

## Repair implemented and verified (synthetic only)

- Discovery now preserves absent/null/value `icon`/`image` via the same `actor_image_uri`
  extraction used by verified Update. Both paths transactionally persist URLs, clear
  changed/removed Paperclip metadata and enqueue bounded profile-media work.
- `src/mastodon/profile_media.rs` shares slot/state/persistence and second-precision
  monotonic generation fencing. `src/worker/profile_media.rs` checks actor identity,
  domain, suspension, URL and generation before fetch and again under row/domain locks
  before install. No REST hotlink or moderation bypass was added.
- Fetches use `RemoteFetcher`, including policy checks **before every redirect hop**,
  normal SSRF/DNS/timeout/size controls, and installation-time checks for every visited
  domain. Cross-origin CDN URLs and `.blob` paths are supported; decoded bytes determine
  image type. Limited federation requires both actor and image/CDN domains to be allowed.
- Downloads are limited to less than 8 MiB, four attempts, and the worker Media resource
  class. Profile GIF preparation has frame/cumulative-pixel/encoded-output bounds and
  runs off the async runtime. Verified Paperclip writes repair truncated originals;
  GIF cache checks conservatively reprepare static derivatives because those have no
  separate SQL size/hash metadata. Durable, row-aware reconciliation handles removals,
  replaced paths and ambiguous commits.
- Image persistence uses existing account **UPDATE** privileges after insertion.
  No account INSERT/UPDATE grant delta, description write, operational migration,
  public endpoint, `web.rs`, or preflight/grant-script edit is required by this repair.

### Operator recovery

Using the instance's ordinary administrator configuration (not an actor URL supplied
on the command line):

```sh
rustodon admin refresh-remote-account --account-id <EXISTING_REMOTE_ACCOUNT_ID>
```

The command reads the canonical actor ID from the database, resolves it with the
instance actor's signature and the validated resolver, then uses the same upsert/cache
path. It refuses local/missing accounts and changed identity/handle; preserves legacy
keys, keypair rows, relationships and local suspension. It refreshes metadata and
queues eligible cache checks, **not** a synchronous download. Run Pull and Maintenance
workers with the configured media root to install/reconcile files. Discovery/refresh
can repair missing files as well as missing SQL cache metadata; unchanged verified
Updates do not churn healthy caches. Moderation continues to prevent image fetching
and installation.

### isolated-worker evidence

Execution used a task-owned workspace and shared compiler cache. Only tracked
sources were transferred, with the three tracked public symlinks recreated
separately; no Git metadata, build target, environment, instance state, backup,
or unrelated file was transferred.

- Parser red: `cargo test --locked --lib actor_profile_images` failed for absent
  discovery fields; green after implementation.
- CLI red/green: `cargo test --locked --test cli_help remote_refresh`, followed by
  all seven CLI tests.
- Fixture boundary red: disabled shared persistence in the **task workspace only**;
  discovery produced zero instead of two queued images. Restored the implementation
  immediately. Tests were drafted concurrently with the worker; this behavioral
  red run is a regression-boundary check, not a claim that all worker implementation
  was written strictly after those tests.
- Eight focused restricted-writer fixture tests pass: discovery → two queued images
  → decoded/cache metadata → nonplaceholder REST/disk; absent/null and same-URL
  remove/re-add; identity/suspension/reject-media races; actor/CDN allowlists;
  signed canonical refresh preserving local state/keys; missing/truncated PNG and
  GIF files; missing metadata; pre/post-commit ambiguity and physical removal cleanup.
  A temporary filtered worker-harness invocation ran only the profile-media cases
  while retaining normal fixture cleanup:
  `cargo test --locked --features test-support --test workers profile_media:: -- --ignored --nocapture --test-threads=1`.
- Paperclip work-budget/truncated-write tests reproduced failures before hardening;
  all 22 Paperclip tests pass.
- Clean-checkout gates temporarily removed **only the task's** pinned-source symlink:
  `cargo fmt --all --check`; `cargo test --locked --all-targets --all-features`
  (**428 passed, 159 ignored, 0 failed**, 29 targets); and
  `cargo clippy --locked --all-targets --all-features -- -D warnings`.
  Historical external run artifacts are not in the repository. Pinned-source link restored.
- Default-feature production `cargo check --locked --lib --bin rustodon` passes.
  Default-feature `--all-targets` hits the pre-existing
  unguarded `Queue::with_complete_fault()` use in `tests/workers.rs`; the established
  all-feature gate passes. This unrelated test-support issue was not expanded here.
- Independent source review approved the discovery fields and the final worker/
  recovery design after redirect-policy, partial-file and GIF-budget findings were fixed.

The issue remains open for the parent's live deployment/recovery and rendered-media
verification. No live origin, live environment or instance workload was contacted.


## Combined worker-gate follow-up

- The original task's combined worker gate reported **69 passed, 1 failed**. Its
  historical external artifact is not in the repository. The same superseded
  cached avatar path remained after Ingress.
- Root cause is an outdated test execution boundary, not lost cleanup: Update now
  clears stale profile metadata and durably stores the old paths for Maintenance.
  Delete's current-row sweep cannot rediscover those superseded paths. The legacy test
  processed only Ingress and asserted physical cleanup without dispatching/running the
  newly queued Maintenance work.
- The regression now verifies both undispatched cleanup records survive Delete with
  exactly the original avatar/header paths, dispatches through the real outbox and
  executes only Maintenance (no synthetic-origin network requests), checks that cleanup
  is acknowledged rather than retrying/dead-lettered, and retains all physical-file
  deletion assertions. No production cleanup behavior is changed.
- After the test correction, the original unfiltered worker Rust suite passes
  **70/70**, including all eight profile-media cases. The enclosing harness then
  failed its separate worker-readiness phase with `PF_WRITE_DATABASE_PRIVILEGES`;
  this older worktree lacked the parent's grant/startup fixes. No grant changes
  were made for this follow-up.
- Isolated `cargo fmt --all` ran before the successful full worker tests and its
  formatted test source was retrieved. Follow-up Clippy could not be confirmed
  because execution became unavailable after timeouts and a name-resolution
  failure. This is a remaining verification limitation, not a reported Clippy success.
- Independent review approved the test/doc-only correction: durable paths and physical
  cleanup remain asserted; no production change or manual unlink masks lost work.

## Live verification — 2026-09-11 UTC

- Combined isolated-worker gates passed, including all 71 worker tests and startup safety;
  build/deployment evidence is in [the thread repair](repair-remote-reply-thread-persistence.md#combined-isolated-worker-validation-and-live-recovery--2026-09-11-utc).
- Source `b2937cf` was deployed through the reviewed application-only process. No
  profile grants or schema migration were needed.
- The signed DB-canonical account refresh succeeded, and both queued cache checks
  completed using the existing runtime configuration and media volume.
- REST and browser checks confirmed locally cached avatar and header media. The
  avatar was `image/png`, 132297 bytes, and decoded at 400×400; the header was
  `image/png`, 140484 bytes, and decoded at 699×233. Original and static variants
  returned HTTP 200.
- The bundled frontend rendered both images with complete, nonzero dimensions,
  no runtime errors, and no remote-hotlink fallback.
- Account and accepted-follow identities remained unchanged. The historical
  external run artifact is not in the repository. Acceptance is satisfied; issue
  archived.
