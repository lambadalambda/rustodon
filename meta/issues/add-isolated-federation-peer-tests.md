# Add isolated bidirectional federation peer tests

## Summary

Add repeatable Mastodon/Rustodon and Pleroma/Rustodon interoperability scenarios on an isolated worker, complementing—not replacing—the fixture differential suite.

## Requirements

- Start distinct disposable instances with independent databases, signing identities, media, and active federation workers.
- Reuse the existing pinned Mastodon 4.6.5 source/images and fixture lifecycle where practical. Pin Pleroma source/image and dependencies explicitly before claiming its coverage.
- Keep any private-network/test-CA transport capability behind explicit test-only build/runtime boundaries. Never weaken ordinary release SSRF, TLS, signature, origin, redirect, or response-size checks.
- Use fresh cross-peer actors without preseeded actor caches or follows; assert discovery, accepted follows, and push-ingested public statuses in both directions first, then extend lifecycle/privacy coverage.
- A resolver/search fetch must not substitute for successful push ingestion of a status. Check actual remote state rather than treating HTTP 2xx as completion.
- Run all builds, tests, and containers on an isolated worker, with bounded task-owned resources and cleanup; never touch the live tunnel, lain.com, or instance secrets.

## Acceptance Criteria

- A documented command starts isolated peers on an isolated worker and exercises real application GET/signature/inbox/worker paths.
- Each claimed direction/activity passes assertions on actor/object identity and received state; unsupported or blocked scenarios fail or remain explicitly open.
- Test-network routing fails closed for unconfigured destinations and is unavailable to ordinary release builds, with regression tests.
- Cleanup removes only the run's recorded resources; repeated runs do not require production credentials or public origins.

## Notes

- Subissue of [isolated worker parity verification](run-essential-parity-gates-on-isolated-worker.md).
- Existing differential mode compares two implementations under one logical fixture identity and does not run Mastodon Sidekiq; it cannot serve as the peer convergence test unchanged.
- Begin with one thin Mastodon smoke and share the scenario runner with a Pleroma bootstrap rather than building a general orchestration framework.

## First Mastodon smoke foundation — implemented

- Command and boundaries: [docs/federation-peer-smoke.md](../../docs/federation-peer-smoke.md),
  `CARGO_BUILD_JOBS=2 tools/federation-peer-smoke` on an isolated worker in
  a task-owned workspace. The thin sibling sources fixture image
  pins/database lifecycle; it never pulls images or fetches reference source.
- Transport commit `c82a0a9` (`8e980ba` before cherry-pick): debug **and**
  `test-support` gated exact HTTPS `.invalid` origin map plus explicit PEM CA,
  automatically applied across RemoteFetcher constructors/callers. Invalid,
  partial and unmapped configurations fail closed; ordinary builds cannot use
  it. TLS hostname verification, signatures, origin/redirect/response limits
  remain active. No synthetic public DNS answers are used for this path.
- Distinct emptied databases, fresh functional local users/generated tokens and
  keypairs, separate media/origins, Mastodon Puma/Sidekiq and Rustodon web/workers.
  Runtime roles cannot write the other peer's database. Generated CA material
  is deleted; cleanup checks only recorded containers/volume/network and PIDs.
- Ignored integration test covers fresh actor resolution through v1 account
  search, exact actor URI/key persistence, sequential Follow/Accept convergence
  in both databases, and public Create ingestion in both directions. Received
  status checks are SQL-only. A TLS audit also requires the matching signed,
  successful inbox Create and public audience, and rejects GETs of the new
  object URLs; partial audit writes are retried, not mistaken for completion.
- Parent-owned prerequisite fixes used: R04 `25ad8e2` and R05 `e325042`, plus
  cached-image verifier `52fa99a`. No ingestion/parity implementation was repaired
  by the peer harness. R01/R06 were not required for this initial public smoke.

### Historical external-run evidence

All commands below ran in an isolated task workspace with two Cargo jobs; the
machine-local artifacts are not in the repository.

- Transport RED: `cargo test --locked --features test-support --test
  remote_peer_transport -- --nocapture` failed with mapped validation returning
  `Dns`. GREEN: **2 passed**, including TLS/SNI, cryptographically verified signed
  GET/POST, resolver, redirects, limits and fail-closed configuration. Existing
  remote unit tests: **33 passed, 1 ignored**. Feature-disabled and
  `profile.test.debug-assertions=false` transport gates: **2 passed each**.
  Scoped Clippy and owned-file formatting passed; independent review approved.
- Bootstrap RED runs found/fixed only harness assumptions: Mastodon approval
  callbacks, generated Doorkeeper tokens, numeric actor paths, current `keypairs`
  storage, and TEMP privilege required for Mastodon's materialized-view refresh.
- First full live pass: **1 passed in 4.54s**. The audited sequential pass was
  **1 passed in 4.34s**; all eight discovery/follow/public-push/audit direction
  assertions passed and cleanup exited **0**. A final race-corrected repeat was
  **1 passed in 4.23s**; all eight direction assertions passed and cleanup exited
  **0** again. Task cleanup completed successfully.
- Audit extraction TDD: **2 Python tests passed**. The concurrent JSONL reader
  regression: **1 passed, live test ignored**. Scoped harness Clippy with
  `-D warnings` passed.

### Explicitly open / unclaimed

- A later [five-scenario Mastodon run](adapt-and-run-peer-matrix-on-isolated-worker.md#remaining-audit-peer-rerun-2026-09-13)
  supersedes the source-only execution status below: `public`, `privacy`, `notes`,
  `profile`, and `interactions` passed on the recorded combined source. That
  historical run does **not** establish acceptance for the current final tree,
  reply lifecycle behavior, or Pleroma.
- Pleroma bootstrap remains open; this foundation does not attest any Pleroma
  release or image.
- The initial v2 search attempt returned empty account results. Parent source
  repair `0d513f5` existed but was **unexecuted** in this early run. This runner
  retained the v1 resolver and did not claim v2 parity. Deferred v2 gate:
  `tools/mastodon-fixture schema-read-test v2_account_search`.
- Simultaneous reciprocal follows are not claimed: an early historical run
  encountered a pinned Mastodon `account_stats` PostgreSQL deadlock, retry and
  remaining pending follow requests. The bounded first smoke deliberately
  exercises each direction to convergence in sequence; it does not alter
  activity handling or retry policy. Keep concurrency stress as separate
  follow-up work.
- This issue stays open for the remaining acceptance scope. Parent owns issue
  indexes.


## Privacy extension — source implemented, verification pending

- The existing runner now selects `public` (default) or `privacy`; each command
  starts a fresh isolated run. Privacy adds distinct non-following recipient
  and outsider accounts on each side, followers-only/direct received-row and
  signed inbox audience checks, and recipient versus outsider/anonymous REST
  authorization. No canonical status URL is fetched to create received state.
- Audit extraction TDD initially rejected missing audience metadata, then all
  three extraction tests passed. The first live privacy attempt reached both
  successful Follow/Accept directions, then failed with `RowNotFound`, the
  expected RED before observer bootstrap was added.
- Observer bootstrap was then edited, but execution became unavailable after
  timeouts and name-resolution failure. No runtime assertion was disabled and no
  privacy pass was claimed at this stage. A review also requested rejecting any
  Public-addressed attempt for the private object even alongside a valid private
  delivery; that assertion and unit regression were added but not executed in
  this stage.
- Note edit/delete, profile Update, and interaction coverage are subsequent
  bounded extensions. The parent owns ordinary `tag:` atom identifier handling;
  the runner must not rewrite wire identifiers or repair production code.

## Note lifecycle extension — source only, unexecuted

- `tools/federation-peer-smoke notes` selects a fresh, sequential six-minute
  Create → Update → Delete scenario for public and followers-only Notes in both
  directions. Each phase requires exact received identity/state and a matching
  signed successful inbox activity. Update must preserve the received row ID,
  URI and visibility while changing content, warning and edit timestamp.
- Private origin/receiver outsider and anonymous REST access is checked before
  and after editing. Public-addressed private Create/Update attempts fail. Delete
  requires retirement of the positively observed row and author/recipient REST
  denial. Tombstone audience privacy is not claimed.
- No resolver fetch manufactures received state, and canonical status GETs fail
  the audit. No wire metadata is rewritten. Parent `a3fe48a` (local cherry-pick
  `e7e7002`) handles ordinary `tag:` atomUri metadata; its execution is pending too.
- All new lifecycle compilation, formatting/lint, unit and live verification was
  deferred while isolated execution was unavailable. Source TDD was unavailable;
  no lifecycle pass was claimed at this stage. The issue remained open.

## Full profile Update extension — source only, unexecuted

- `tools/federation-peer-smoke profile` keeps the same fresh setup and three-minute
  bound. It PATCHes each peer with text, actor flags, a profile field and both PNG
  uploads, then requires exact received actor identity, rendered note, flags,
  fields and advertised avatar/header URLs plus signed inbox Update evidence.
- Post-mutation actor-URL GETs fail the audit, including a final rescan after both
  directions. The source uploads/descriptions must exist, but remote image
  download and remote image-description persistence are not claimed.
- Multipart source regression, compilation, format/lint, actual upload and live
  propagation had **not run** at this stage because execution was unavailable;
  independent review was source-only and could not establish profile acceptance.

## Interaction extension — source only, unexecuted

- `tools/federation-peer-smoke interactions` adds sequential Like/Undo and
  followers-only Announce/Undo in both directions under a six-minute test bound.
  Each target is a freshly push-ingested public Note; no status resolver is used.
- Like requires the received favourite row and signed activity; Undo is correlated
  to the actual Like ID captured on the wire and must remove the observed row.
  Private Announce requires exact wrapper URI, actor, public-original target and
  the boosting actor's canonical followers audience, with recipient access and
  outsider/anonymous denial. Here the recipient is both original author and an
  established follower. Undo retires that wrapper, not the public original.
- Audits now retain activity IDs and separate outer audiences. A private Announce
  may embed a public Note without becoming public itself; any Public-addressed
  Announce attempt still fails. Undo's outer audience is unconstrained. Private
  attempts and canonical status GETs are rescanned at scenario end, including
  delayed events from earlier cases. The privacy suite receives the same final
  audit rescan. Counter/notification parity and concurrency stress are unclaimed.
- Review corrections bind Announce ID and audience to one signed successful event,
  with regressions rejecting split evidence and accepting a later exact event.
  The proxy records attempts before backend forwarding (null status), then response
  status separately, so backend failures cannot hide Public-addressed attempts.
  Mocked actual-forward failure regressions and these source fixes are unexecuted.
- Selected ignored tests are checked in the compiled test list before resources
  start, preventing missing/renamed scenarios from silently passing zero tests.
- These audit/schema/helper changes, new unit cases and all interaction execution
  were **pending** at this stage. Earlier three passing Python audit tests predated
  the new activity-ID/envelope fields. No compilation, formatting/lint, unit tests
  or live scenarios were performed during the source-only continuation.

## Historical deferred execution checklist

This checklist was later executed for the five Mastodon scenarios in the linked
[2026-09-13 run](adapt-and-run-peer-matrix-on-isolated-worker.md#remaining-audit-peer-rerun-2026-09-13):
with `CARGO_BUILD_JOBS=2`, rerun Rust and Python unit regressions, formatting and
scoped Clippy, observer bootstrap, and all five fresh commands (`public`,
`privacy`, `notes`, `profile`, `interactions`), then repeat successful live runs
to verify cleanup. That superseding result remains historical combined-source
evidence, not current-final-tree, reply-lifecycle, or Pleroma acceptance.

## Pleroma quota decision

Pleroma remained blocked by Docker Hub's anonymous pull quota; no pinned Pleroma
build or peer result is claimed. See the public
[build record](../../docs/federation-pleroma-build.md).
