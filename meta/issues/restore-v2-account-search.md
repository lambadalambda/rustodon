# Restore v2 account search for clients

## Summary

The real peer smoke found that `/api/v2/search` always returns an empty accounts array, even though the v1 account-search route already supports cached/local search and authenticated remote resolution. Mainstream clients using v2 cannot discover accounts through that route.

## Requirements

- Return account search results for omitted type or `type=accounts`, while preserving hashtag results and current status-search limits.
- Reuse the existing account search/resolution policy rather than duplicate networking, domain checks, shared limits, or signing logic.
- Keep unauthenticated requests from triggering remote writes or resolution; preserve endpoint-specific scopes, filtering, pagination, and response shape.
- Do not expand this into a new full-text status search backend.

## Acceptance Criteria

- HTTP regressions cover local and cached account results, type separation, pagination, and scope failures.
- Authenticated remote-handle resolution uses the existing safe fetch/persistence path; unauthenticated resolve cannot initiate outbound requests.
- Applicable client and differential gates pass in the supported isolated
  execution environment and behavior limits are documented.

## Notes

- Discovered by the Mastodon peer smoke, which currently uses v1 account search instead.
- Source confirmed in `src/web.rs::search_v2`; implementation and its named HTTP
  regression are present and passed on the isolated worker. Broader client and
  differential evidence remains pending.
- At implementation time isolated worker was unreachable, so the user requested
  continuing source work without local build/test fallback. That historical
  verification state is superseded in part by the later isolated-worker evidence below.

## Status — OPEN: named HTTP regression green; broader client/differential evidence pending

- Extracted the v1 handler's account loading, domain policy, freshness check, shared
  resolution limits, instance signing, safe remote fetch and persistence into one
  `search_accounts` helper. V1 retains required `read:accounts` authentication and
  its parameter parsing; v2 retains optional-owner `read:search` and its existing
  type, limit, offset and hashtag contracts.
- V2 returns accounts only for omitted type or `type=accounts`. The shared helper
  disables resolution without a resource owner, including application-only and
  unknown-token requests treated as anonymous by the existing optional-auth policy.
  Anonymous callers can search stored accounts, but cannot fetch or persist remote
  actors. Following filtering remains in the existing read-only account loader for
  owners; ownerless `following=true` explicitly returns no accounts rather than
  falling through to the loader's unrestricted anonymous search.
- Hashtag selection/serialization is unchanged. Status and collection search remain
  empty; this is not a full-text status-search implementation or full v2 parity claim.
- No browser settings, `write_repository.rs`, peer tests, issue indexes, or
  unrelated fixture infrastructure changed. The harness exposes the named
  `schema-read-test v2_account_search` target; the current default schema
  aggregate also includes it among its 12 selectors.

### Initially unexecuted source-first regressions

`src/web/account_search_tests.rs` was written before the production refactor. It is
an ignored HTTP/router fixture test compiled only with `test-support`, using the
existing private remote-fetch endpoint hook rather than a new production injection API.
Assertions cover:

- Local and cached-remote account results, anonymous/authenticated, omitted and
  accounts-only type; comparison with v1's serialized results.
- Explicit type pagination against v1, following-only results, zero limit, existing
  ignored offset for untyped search, anonymous pagination denial, bad inputs and
  endpoint-specific scope failures.
- Hashtag-only, status-only and unknown types do not return accounts; hashtag results
  remain identical between typed and untyped searches.
- A counting loopback mock sees no fetches and the database gains no remote rows for
  anonymous, application-only, unknown-token, non-account type, nonzero-offset or
  zero-limit resolution requests.
- Authenticated exact-handle resolution performs WebFinger and signed actor GET,
  persists the account and returns it. Fresh-cache and anonymous subsequent reads
  must return the same account without more fetches. The fixture uses the existing
  HTTP `.onion` resolution path routed explicitly to loopback (no Tor/DNS), because
  the synthetic endpoint hook does not change HTTPS or install a test CA. It proves
  no HTTPS trust behavior. The actor uses a literal public inbox address for
  URL/address validation only; it is never contacted.

### Independent source review

Read-only correctness and architecture/DRY review approved the final scoped source.
It initially caught two issues: ownerless `following=true` could fall through to an
unrestricted anonymous search, and an HTTPS resolver URL could not use the plain
HTTP mock. Both were corrected before commit: the shared helper now returns no
accounts for ownerless following-only requests, and the mock uses the existing
HTTP `.onion` path with explicit loopback routing. Final review found no remaining
substantive source-level blockers. This is not compilation or runtime evidence.

### Historical initial verification limits

TDD RED/GREEN execution was infeasible because isolated worker DNS was unavailable. **No new
build, test, formatter, lint or container command has run**, locally or remotely; no
execution retry was attempted. The new regression has not even been compiled. Formatting
and test assumptions require remote verification before closing this issue.

At that time, the queued isolated worker command set (now historical, not current
execution guidance) was:

```sh
# Pending, not execution evidence:
tools/mastodon-fixture schema-read-test v2_account_search
cargo test --locked --lib
cargo clippy --locked --all-features --lib -- -D warnings
rustfmt --edition 2024 --check src/web.rs src/web/account_search_tests.rs
sh -n tools/mastodon-fixture
tools/mastodon-fixture schema-read-test
```

The later required `core_rest_serializers` differential invocation covers v1
account search and hashtag behavior and passed. It does not exercise v2 account
results. A dedicated v2 account-result differential/client case and peer use of
v2 discovery remain pending.

The intended original behavioral RED was the named v2 regression against the
pre-refactor handler: its local-account assertion should fail because `accounts`
was hard-coded empty. That boundary was source-predicted at the time, not newly
executed as a historical mutation.

The prescribed and external read-only pinned Mastodon 4.6.5 checkouts were
unavailable during this follow-up; expected revision
`1440d55b139e39ec722c2a3db7f60b66cd889048`. No replacement was fetched and no
fresh upstream/differential comparison is claimed. Fixture-owner
setup/persistence will not itself establish production-role permissions.

### Later isolated-worker evidence

The later test-audit run superseded the initial compile/runtime uncertainty:
`tools/mastodon-fixture schema-read-test v2_account_search` passed as one of the
12 independently bounded final schema selectors, and the final combined tree's
ordinary default/debug-all-feature/release tests, formatting, and strict Clippy
checks passed. See `../../docs/testing.md` and its historical external results.

The peer runner intentionally still uses v1 account discovery. A dedicated
v2-search differential/client run and fresh peer use of v2 discovery have not
been claimed, so this issue remains open rather than treating the named HTTP
regression as complete client acceptance.


## Status 2026-09-23

Live anonymous `GET /api/v2/search?q=lain&type=accounts` on rustodon.social returns
local and cached remote accounts. Only the real-client check remains; the user
will do it with the mobile-client run.
