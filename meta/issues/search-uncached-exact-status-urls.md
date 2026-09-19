# Resolve uncached exact status URLs in search

## Summary

Bounded follow-up to [Restore status search in the bundled frontend](restore-frontend-status-search.md), based on `54dc5c9`.

## Requirements

- Authenticate read/read:search and require resolve=true before uncached exact HTTP(S) status resolution; preserve account URL search semantics and all early no-fetch gates.
- Distinguish unknown persisted targets from denied authoritative targets; never fetch to bypass a known denial.
- Reuse signed RemoteFetcher, actor resolver, existing Note/Question parsing and apply_remote_note_create with delivery_target_account_id=None. Never manufacture an audience or mention for the searching user.
- Preserve origin/identity, redirect, SSRF/TLS/size/signature, deletion/tombstone, author and domain policy boundaries. Avoid writes where rejection is knowable first.
- Reuse a narrow service helper if needed, not the Announce workflow or wholesale worker internals. No schema, jobs, privileges, or new source identity scheme. Stop for scope checkpoint if more than three subsystems are required.
- Support canonical URLs and only bounded existing display-URL discovery. Without a trusted existing Link metadata helper, HTML discovery is explicitly unsupported.

## Acceptance Criteria

- TDD signed-transport fixture covers public/unlisted/private visibility, no fabricated mention, durable missing-parent resolution, zero-fetch cache reuse, malformed/mismatched/redirected identity and policy denials, including no known-denied bypass.
- Restricted runtime/writer credentials (owner only for setup); bounded unique disposable NAS7203 codec/PostgreSQL 14 resources, no production or browser lane.
- Read-only exact pinned Mastodon source contracts; record actual red/green evidence and deferred HTML/full-text boundaries in DEVLOG.
- Independent parent review, maximum two substantive rounds; leave changes uncommitted.

## Status

Open: implementation and focused NAS red/green verification complete; parent review 1 (`a30e`) reported no blockers/high findings. Its directly relevant medium test-isolation gap is covered by the verified isolated policy matrix; review 2 remains pending. Leave uncommitted.

## Evidence and boundaries

- Actual pre-implementation RED: uncached public URL returned zero results.
- Final `web::account_search_tests::` GREEN: 4 passed using restricted runtime/writer roles, PG14 and bounded NAS7203 resources. Final library Clippy, local fmt/diff checks pass. See DEVLOG and ignored `target/uncached-search-evidence/` for commands/logs.
- Supports exact canonical Note/Question HTTP(S) URLs and same-origin redirects to exact canonical IDs, plus previous cached URI/display/local aliases. HTML discovery and unredirected display responses with different IDs remain unsupported; full-text, browser, collections and Create/Announce wrapper discovery are deferred.
- Existing account-handle resolution regression passes with restricted writer credentials. Actor-URL dispatch is not newly implemented by this status-only resolver.
- Missing-parent durable outbox emission is verified, not worker execution. Signed HTTP fixture verifies RSA signatures; live TLS/peer gates were not run.
- Parent review 1 (`a30e`) reported no blockers/high findings. Its medium policy-test isolation gap is addressed below; parent review 2 remains pending.

## Review 1 follow-up

- Test-only scope: isolate uncached viewer block, reverse block, mute, suspended-author and viewer-domain-block policies. Each case must assert empty results, no persisted status, exact fetch boundary (domain zero; author policies one object fetch), and cleanup before the next case. No implementation, HTML or actor-URL changes.
- Final restricted Linux PG14 suite: 4 passed (28.23s), `review1-verified.log`. Library Clippy and local fmt/diff checks pass; broader test Clippy has only existing unrelated diagnostics. Final source hashes match NAS and production sources are unchanged from pre-review evidence. Task resources and both anonymous volumes removed/absence verified. See DEVLOG and `target/uncached-search-evidence/`.
