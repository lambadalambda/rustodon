# Accept instance activity in the bundled browser

## Current disposition — accepted after parent review 980993

Accepted and archived on parent approval. Review `980993` found no high-severity
blocker and supports focused closure from the combined reviewed recording,
cache, pinned-source, restricted PostgreSQL/worker/main-HTTP evidence and actual
browser acceptance on exact app `10ddf66caf1fe8714f7c41ac4a1a0b8ceaf7fe34`.
This disposition supersedes historical open, review-pending, browser-deferred and
leave-uncommitted statements below; earlier logs remain historical evidence,
not fresh execution claims. The wider combined acceptance sweep remains pending.

### Review follow-up notes (nonblocking; deferred)

- Medium: the inherited launcher cleanup/status handling can mask a cleanup
  failure. This run also has separately executed final cleanup verification
  (`final-cleanup-proof.txt`) confirming owned containers/network and generated
  credentials/media/build output absent. Harden failure propagation separately;
  do not infer that every future run is clean from the acceptance exit code alone.
- Medium: negative observer unit coverage is missing (e.g. malformed/missing
  response/count observations). Actual network/render acceptance passed; add
  focused negative observer tests in later harness work, not this closure.
- Parent reported the two new Python tests passing. Closure reruns the two new
  and eight shared tests only; no heavy lanes or application edits. Reference
  subset source check and ignored library tests retain their recorded boundaries.

## Summary

Bounded final browser acceptance for [Compute real instance activity metrics](compute-real-instance-activity-metrics.md) on exact application source `10ddf66caf1fe8714f7c41ac4a1a0b8ceaf7fe34`.

## Requirements

- Minimally reuse the pinned-frontend browser harness and shared browser helper; no new framework or production changes.
- Use serial, resource/time-bounded task-only PostgreSQL 14, media, accounts and restricted runtime/writer roles with migrations 1–6. Preserve source/image hashes, role grants and sanitized command logs.
- Prove empty history and eligible authentication today both publish zero. Seed simulated historical membership via the real recording helper, moving its date only in fixture setup; never claim observed midnight rollover or real historical backfill.
- Observe nonzero public sidebar, reload, real endpoint fetches and initial frontend metadata agreeing with v2 monthly and NodeInfo 4/24-week counts. No DOM/response injection.
- Ordinary browser evidence is required; limited federation suppresses v2/initial metadata but retains raw NodeInfo counts (earlier HTTP coverage may support this; native DOM mode check if feasible).
- Run only relevant existing source contracts/library checks, not the full matrix. Clean task-owned resources and credentials. Leave substantial harness edits uncommitted for parent review.

## Acceptance Criteria

- Actual browser screenshots and network/DOM assertions demonstrate the count, including after reload, on the exact-source binary.
- Evidence records fixture seeding, safety boundaries, executed gates, source provenance, grants and cleanup honestly.

## Status and evidence

Final fresh uninterrupted browser gate **passed**, including the optional actual
limited-mode sidebar and reload. Open only for independent parent review and the
parent's combined acceptance sweep; all changes intentionally uncommitted.

- Empty history and real confirmed login today: v2/initial/NodeInfo **0**.
- Real login helper created today's Alice membership; setup moved its existing
  bucket/member date to yesterday. Simulated historical ordinary sidebar,
  v2/initial monthly and NodeInfo 4/24-week values **1**, including reload.
- Limited sidebar/v2/initial **0**, NodeInfo **1/1**, including reload. Real
  frontend XHR and DOM waited for independently on every public document.
- Exact default-feature binary SHA-256
  `571f4b8c6d33ab4dad83eb631a7c40ab44ef7c5713f1b5e2441228519d239e7c`.
  All 6,480 tracked source files/symlinks matched the immutable app archive;
  pinned frontend checksums and reference source verified. PG14.23 migrations
  1–6, role flags/memberships, actual runtime/writer attachments and grants saved.
- Focused library **3 passed / 7 ignored**, existing activity source contract
  **1 passed** (verified reference subset, not full source lane), new harness
  regressions **2 passed**, shared regressions **8 passed**, diff check passed.
- Initial missing-module TDD red retained. First browser pass retained separately;
  final run adds stricter limited-render waiting and focused checks. Preparation
  caught an ambiguous replacement; corrected before final execution. Initial
  all-source manifest mistakenly compared locally edited issue metadata;
  corrected to immutable Git objects and all-source comparison passed, without
  modifying the executed app source.
- Task resources, generated credentials/keys/session, database/media and binary/
  build output cleaned. Exported evidence sanitized and checksums verified.
- Evidence: `target/activity-browser-evidence/evidence/` and NAS
  `/srv/workspaces/rustodon-activity-browser-10ddf66-alice/evidence/`.
  [Exact recipe, bounds and exclusions](../../tools/activity-browser/README.md).
- No production/backfill/deployment, clock override, real UTC rollover, main HTTP
  rerun or full matrix claim. Independent delegation failed at the nesting limit;
  **zero independent review rounds claimed**. Parent review is still required.
