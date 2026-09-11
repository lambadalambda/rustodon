# Add isolated bidirectional federation peer tests

## Summary

Add repeatable Mastodon/Rustodon and Pleroma/Rustodon interoperability scenarios on Secunda, complementing—not replacing—the fixture differential suite.

## Requirements

- Start distinct disposable instances with independent databases, signing identities, media, and active federation workers.
- Reuse the existing pinned Mastodon 4.6.5 source/images and fixture lifecycle where practical. Pin Pleroma source/image and dependencies explicitly before claiming its coverage.
- Keep any private-network/test-CA transport capability behind explicit test-only build/runtime boundaries. Never weaken ordinary release SSRF, TLS, signature, origin, redirect, or response-size checks.
- Use fresh cross-peer actors without preseeded actor caches or follows; assert discovery, accepted follows, and push-ingested public statuses in both directions first, then extend lifecycle/privacy coverage.
- A resolver/search fetch must not substitute for successful push ingestion of a status. Check actual remote state rather than treating HTTP 2xx as completion.
- Run all builds, tests, and containers on `lain@secunda.local`, with bounded task-owned resources and cleanup; never touch the live tunnel, lain.com, or instance secrets.

## Acceptance Criteria

- A documented command starts isolated peers on Secunda and exercises real application GET/signature/inbox/worker paths.
- Each claimed direction/activity passes assertions on actor/object identity and received state; unsupported or blocked scenarios fail or remain explicitly open.
- Test-network routing fails closed for unconfigured destinations and is unavailable to ordinary release builds, with regression tests.
- Cleanup removes only the run's recorded resources; repeated runs do not require production credentials or public origins.

## Notes

- Subissue of [Secunda parity verification](run-essential-parity-gates-on-secunda.md).
- Existing differential mode compares two implementations under one logical fixture identity and does not run Mastodon Sidekiq; it cannot serve as the peer convergence test unchanged.
- Begin with one thin Mastodon smoke and share the scenario runner with a Pleroma bootstrap rather than building a general orchestration framework.

## First Mastodon smoke foundation — implemented

- Command and boundaries: [docs/federation-peer-smoke.md](../../docs/federation-peer-smoke.md),
  `CARGO_BUILD_JOBS=2 tools/federation-peer-smoke` on `lain@secunda.local` in
  `/home/lain/rustodon-parity/peer-tests`. The thin sibling sources fixture image
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

### Remote evidence

All commands below ran in the prescribed Secunda workspace with two Cargo jobs.

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
- First full live pass: `target/peer-260149/smoke.log`, **1 passed in 4.54s**.
  Audited sequential pass: `target/peer-288879/smoke.log`, **1 passed in 4.34s**,
  all eight discovery/follow/public-push/audit direction assertions passed;
  cleanup exit **0**. Full command logs: `target/peer-smoke-seventh.log` and
  `target/peer-smoke-audited-sequential.log`. Final race-corrected repeat:
  **1 passed in 4.23s**, `target/peer-300477/smoke.log`, command log
  `target/peer-smoke-final.log`; all eight direction assertions and cleanup
  exit **0** again. The outer remote shell continued after the harness.
- Audit extraction TDD: `target/peer-audit-{red,green}.log`, **2 Python tests
  passed**. Concurrent JSONL reader regression:
  `target/peer-audit-reader-{red,green}.log`, **1 passed, live test ignored**.
  Scoped harness Clippy with `-D warnings`: `target/peer-harness-clippy.log`.

### Explicitly open / unclaimed

- Pleroma bootstrap and the broader lifecycle/privacy/media matrix remain open;
  this foundation does not attest any Pleroma release or image.
- Rustodon `/api/v2/search` currently hardcodes empty account results. Both peers
  use the actual v1 account-search resolver here; v2 parity is not repaired or
  claimed. Evidence: `target/peer-254966/`, `src/web.rs::search_v2`.
- Simultaneous reciprocal follows are not claimed: `target/peer-271058/` records
  a pinned Mastodon `account_stats` PostgreSQL deadlock, retry and remaining
  pending follow requests. The bounded first smoke deliberately exercises each
  direction to convergence in sequence; it does not alter activity handling or
  retry policy. Keep concurrency stress as separate follow-up work.
- This issue stays open for the remaining peer matrix. Parent owns issue indexes.


## Pleroma quota decision

The exact v2.10.2 source archive and guarded build helper are prepared, but the
required release base image pull failed under Docker Hub's anonymous quota.
The user chose to leave Pleroma blocked until that quota resets rather than
configure registry authentication. Do not claim Pleroma coverage or retry in a
loop. See [build evidence](../../docs/federation-pleroma-build.md).
