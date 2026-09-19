# Search known exact status URLs

## Summary

First bounded slice of [Restore status search in the bundled frontend](restore-frontend-status-search.md), based on f93b6ef. Return already persisted, eligible exact status URLs without remote fetching or Elasticsearch.

## Requirements

- Follow pinned Mastodon 4.6.5 SearchService URL-branch semantics: resolve=true; authenticated user and read/read:search scope; exclusive URL branch; absent/blank/statuses type; zero limit returns none; positive offset suppresses results only with a specified type.
- Match local permalink/AP object URLs at the configured origin or exact persisted statuses.uri/url, using normal StatusAccess and authorized_status projection. Never create statuses or fetch remote URLs.
- Enforce audience membership, deleted/suspended visibility and explicitly stricter search suppression for viewer blocks, mutes and blocked domains. This is an intentional safety policy, not bug-for-bug parity with reference context silencing.
- account_id/min_id/max_id/following constrain the text branch, not the exact URL branch, consistent with the reference.
- Anonymous resolution or pagination returns 401; preserve existing account-search behavior otherwise.
- No new schema, protocol, jobs, privileges, or authorization architecture.

## Acceptance Criteria

- TDD pure parsing/branch contracts and persisted restricted-HTTP tests cover known local/remote URLs, hidden/private/direct/blocked/muted/deleted statuses, types, offset, limit, authentication and scopes; reuse account-search fixtures with separate status assertions.
- Verify clean pinned reference revision 1440d55b139e39ec722c2a3db7f60b66cd889048 and inspect SearchService.
- Run focused gates on disposable LinuxNAS7203 PG14 resources with explicit bounds; preserve account-search regression coverage. Record exact executed tests and gaps in DEVLOG.md.
- Independent correctness/architecture review; leave changes uncommitted.

## Exclusions

Uncached resolution and browser regression are next slices. Parent issue stays open; no full fixture matrix or production operations.

## Verification and remaining work

- Implemented read-only lookup and normal authorized projection. NAS focused contracts/account/status HTTP gate passes 3/3; production-library Clippy passes. Restricted read role rejects status UPDATE with SQLSTATE 42501. Exact commands/evidence boundaries are in DEVLOG.md.
- Parent supplied independent review `6a18159b`; its high-severity identity collision is fixed with persisted red/green evidence. Keep this subissue open and uncommitted for review 2 (nested delegation is unavailable here).
- Resumed after SSH unlock. Final focused gate passes 3/3, web unit tests 91 passed/2 ignored, production-library Clippy and fmt/diff checks pass. Broader test-target Clippy remains blocked by unrelated existing warnings; see DEVLOG.md.
- Collected NAS logs and verified main/NAS source hashes. Removed task-only PG/runner containers, both recorded anonymous PG volumes and `status-search-alice` network; retained workspace/evidence and shared caches. No production resources were used.

## Review 1 follow-up

Parent review `6a18159b` found a high-severity identity collision: the combined
URI/display/local-alias lookup selected the lowest ID and could substitute a
foreign display URL for the authoritative status. Fix within this slice only:
validated local alias first, then canonical URI, then display URL. Authoritative
but denied targets must never fall back to colliding rows; ambiguous display-only
matches fail closed. Add persisted collision regressions before implementation.
No ingestion, schema, resolver, or broader identity rewrite.

### Review 1 outcome

Identity selection now uses explicit local-alias → canonical-URI → display-URL
priority before access filtering. Only a unique best-tier match is selected;
ambiguous display-only URLs return no result even if one candidate is hidden.
Persisted tests exercise the three local aliases and canonical URI against an
older foreign display URL, hidden colliding rows, deleted authoritative targets
(no fallback), a local alias versus a foreign URI, and duplicate display URLs.
No schema, ingestion, resolver, or privilege changes were needed.
