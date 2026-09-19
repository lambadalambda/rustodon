# Bounded status-search browser acceptance

## Current disposition — post-0994711

Accepted and archived on parent approval. Accepted parent review `browserreview8e75` found no high-severity blocker; harness committed in `0994711`. Final fresh uninterrupted real-input controller passed Posts, exact navigation/reload, cached additional fetches 0, one verified signed TLS public GET, denied empty results and no private persistence/mention. Nonblocking reviewer follow-ups: All-tab empty results were observed, not explicitly asserted; exactly one public fetch was observed, not hard-asserted. Record these without expanding this slice. Main coordinator reruns accepted: local 4 passed / 1 pinned-source skip; Linux pinned-source 5 passed. These are coordinator-reported results, not new runs by this docs-only task.

This disposition supersedes historical open/review-pending/leave-uncommitted
statements below. Original requirements, hashes, timings and red/green records
are retained as historical evidence, not fresh current-tree gate claims.

## Summary

Browser acceptance of exact application c3497f5cdf71678c70542689e079ac589de253a1, following [Restore frontend status search](restore-frontend-status-search.md) and its known/uncached URL slices.

## Requirements

- Reuse the existing isolated NAS pinned-frontend remote-browser harness; do not change media acceptance or introduce a generic framework.
- Verify clean Mastodon reference 1440d55b139e39ec722c2a3db7f60b66cd889048 and exact app source; PG14 migration 5 with narrow runtime/writer roles.
- Actual signed-in frontend input, Posts result, exact permalink navigation and reload for known canonical status URL; controlled TLS canonical uncached remote Note with valid actor/ID if feasible.
- Observe real frontend resolve=true and supported type/pagination requests without response injection. Representative private/hidden URL displays no unauthorized result; assert no new persistence/mention. Check cached repeat source fetch count if feasible.
- Explicit sequential CPU/memory/PID/wall-time bounds, disposable task-only identities/resources, sanitized evidence and session teardown. No production, deployment, push or app bug fixes.
- Independent review of substantial harness; leave changes uncommitted.

## Acceptance Criteria

- Final focused controller runs uninterrupted, with sanitized request/DOM/navigation/reload and source-count evidence, plus owned-resource cleanup proof.
- Record executable coverage versus actually executed gates. Full-text indexing, HTML alternate discovery, actor URL resolution, hashtag work and full fixture matrix are excluded.

## Status

Open for independent parent review; final fresh uninterrupted controller passed. Delegation from this session was rejected at the nesting limit, so independent review is not claimed. All changes remain uncommitted.

## Evidence

- Exact app and clean pinned source verified. PG14.23/migration5, restricted runtime/writer, pinned existing NAS tools/browser and frontend packs.
- Real signed-in input/Posts, known-local and controlled canonical uncached public result, exact status click/navigation/reload passed. Actual XHRs show resolve=true, limit=11, absent offset, omitted type and type=statuses. No response injection.
- One cryptographically verified signed TLS public GET; cached repeat zero additional fetches. Known hidden URL zero fetches and zero result. Uncached private-to-Carol Note fetched twice (All/Posts) but never persisted/displayed; no task mentions or change in total mentions/known-hidden row.
- Owned browser/session, containers, PG volume/network, media, certs/keys/env and build output cleaned. Sanitized evidence/checksums: `target/status-search-browser-evidence/evidence/`; full commands/bounds in [harness README](../../tools/status-search-browser/README.md).
- Five helper/crypto/focused pinned-source contract tests and eight unchanged media-harness regressions pass. First wrong-signer harness failure and earlier pass retained separately; final evidence is from a fresh complete run after corrections.
- Singleton exact URL yields no load-more control. Nonzero pagination browser execution is not claimed; focused source test checks expansion and Rails URL offset rules. Full Rust pinned-source lane, HTTP security matrix rerun, full browser/DB/worker/peer/differential/check/Clippy gates not run. Full-text/HTML alternates/actor URLs/hashtags remain outside scope.
