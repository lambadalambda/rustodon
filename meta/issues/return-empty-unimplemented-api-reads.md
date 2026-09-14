# Return empty results for unimplemented frontend API reads

## Summary

The user requests empty results instead of the current API 404 responses that repeatedly surface errors in the frontend.

## Requirements

- Inventory unimplemented API read endpoints and their expected frontend response shapes before changing behavior.
- Return endpoint-appropriate empty representations for supported fallback reads, preserving authentication/scope checks.
- Do not hide genuine missing-resource errors or report success for unsupported mutation operations.
- Keep non-API routes and federation protocol behavior unchanged.

## Acceptance Criteria

- Representative frontend polling/list requests no longer return implementation-placeholder 404 errors.
- Tests cover empty response shapes, applicable authorization and retained real-resource/mutation 404 behavior.
- Applicable checks pass on an isolated worker; live verification follows deployment.

## Notes

- Added during thread/profile-media repairs; keep this change and commit separately scoped.

## Owned implementation scope

- Execution used a task-specific isolated-worker workspace with two Cargo jobs.
- Nine GET groups: trends tags/links/statuses, v2 suggestions, directory,
  link timeline, user domain blocks, instance domain blocks, familiar followers.
- An external read-only run inspected pinned Mastodon 4.6.5 source at revision
  `1440d55b139e39ec722c2a3db7f60b66cd889048`. Controllers:
  `api/v1/trends/*`, `api/v2/suggestions`, `api/v1/directories`,
  `api/v1/timelines/{link,topic}`, `api/v1/domain_blocks`,
  `api/v1/instances/domain_blocks`, `api/v1/accounts/familiar_followers`;
  REST familiar-follower/domain-block serializers and DomainBlock model.
- User domain blocks accept `follow`, `read`, or `read:blocks`; familiar
  followers accept only `read` or `read:follows` (not legacy `follow`).
- Reuse repository moderation reads rather than masking actual blocks.
  Instance publishing/rationale/obfuscation controls remain enforced; a
  disabled or unauthorized-to-view published list returns `[]` instead of the
  upstream feature-disabled 404, never exposing private moderation notes.
- TDD: new focused route and disposable-schema HTTP regressions precede handlers.
  No blanket changes to `web_fallback` or mutations.

## Response contracts and deliberate limits

| GET path | Response | Authentication |
| --- | --- | --- |
| `/api/v1/trends/tags` | `[]` (disabled) | Optional token, no scope required |
| `/api/v1/trends/links` | `[]` (disabled) | Optional token, no scope required |
| `/api/v1/trends/statuses` | `[]` (disabled) | Optional token, no scope required |
| `/api/v1/directory` | `[]` (disabled) | Optional token, no scope required |
| `/api/v1/timelines/link` | `[]` (disabled) | Optional `read` / `read:statuses`; existing topic-feed settings can require a user |
| `/api/v2/suggestions` | `[]` (disabled) | User + `read` / `read:accounts` |
| `/api/v1/accounts/familiar_followers` | `[{"id":"…","accounts":[]}]` per distinct requested ID; no IDs gives `[]` | User + `read` / `read:follows`, **not** `follow` |
| `/api/v1/domain_blocks` | Real owner-scoped domain strings, or `[]` | User + `follow` / `read` / `read:blocks` |
| `/api/v1/instance/domain_blocks` | Real publishable blocks, or `[]` when publishing is disabled/not visible | Optional token; user-only publishing/rationale enforced |

- All nine have explicit GET and trailing-slash aliases; Axum supplies HEAD.
  Inventory grows from 106 to 115 method/path contracts. Seven are explicitly
  `DisabledResponse`; the two moderation reads are `Implemented`.
- Collection-only compatibility deliberately differs from upstream disabled
  directory, unavailable link-preview and hidden instance-block-list 404s.
  Familiar-follower IDs are placeholders, not real account-existence results;
  this does not implement batch account lookup or the mutual-follower graph.
- Empty results have no pagination Link. Real user domain-block pages preserve
  descending association-ID cursors and expose Link only for nonempty pages.
- Instance lists exclude noop/unknown severities, preserve obfuscation and
  SHA-256 domain digest, gate public rationale separately, never serialize
  private comments, and use conservative `private, no-store` caching.
- Known remaining unimplemented reads/404s are deliberately deferred:
  `/api/v1/tags/{id}`, `/api/v1/instance/privacy_policy`,
  `/api/v1/instance/terms_of_service`, `/api/v1/annual_reports/{year}`,
  real `/api/v1/accounts` batch lookup, and account endorsements (parent
  validation required). This is not an exhaustive implementation of every
  optional Mastodon API. Unknown API routes and genuinely missing account/status
  resources retain their errors. Unsupported API writes and web/settings
  routes are unchanged; no blanket `web_fallback` rewrite.

## Reproducing the focused HTTP fixture

The permanent harness now selects this ignored HTTP regression directly, using
its normal pinned fixture, least-privilege roles, and task-owned cleanup:

```sh
tools/mastodon-fixture schema-read-test api_empty_reads
```

Run it on an isolated Linux worker, not the local coding machine. No temporary harness or upstream media checkout is needed.
The historical evidence below used the earlier temporary selector and is not
rewritten as evidence of the new command.

## Verification evidence

Historical external evidence; artifacts are not in the repository:

- A historical external RED recorded the new route test failing on absent `/api/v1/trends/tags`.
- A historical external RED recorded the new HTTP test failing on the unregistered route response
  lacking API CORS finalization (before handlers existed).
- A historical external GREEN recorded explicit route contracts passing.
- A historical external GREEN recorded the disposable schema HTTP regression passing (1 test), including
  real moderation rows, empty shapes/cursors, auth/scope/app-only rejection,
  trailing slash/HEAD, and missing-resource/mutation error preservation.
- A historical external run recorded `cargo test --locked --all-targets --all-features` passing
  without the workspace's pinned-source symlink; ignored integration gates
  are not claimed by this run.
- Independent read-only review completed: no confirmed handler defect;
  requested reproducible fixture invocation is documented above. HEAD rejection
  and user-only rationale coverage were added. Whole-list repository loading
  before user-domain pagination remains a deferred performance limitation.

Issue remains open pending parent integration/deployment and live frontend
verification. No live environment or container was modified.

Review follow-up found an in-scope publishing-auth mismatch: upstream permits
functional moved users to view user-published instance blocks/rationale, unlike
`require_user!` used for mutations. The historical external RED reproduces the incorrect
403. A read-specific `functional_or_moved?` eligibility predicate now reuses the
existing OAuth facts query (without decrypting OTP secrets or weakening shared
authentication); ineligible users get the hidden `[]`/null rationale rather than
an extra user-required error. The expanded historical external GREEN passes moved-user and
disabled-user publishing cases, HEAD rejection, mixed public-list/user-only
rationale, and query-bearer injection. Suspended-token rejection remains in the
existing authenticator.

Final historical external gates `cargo fmt --all --check` and
`cargo clippy --locked --all-targets --all-features -- -D warnings` pass.
The historical external all-target run totals **423 passed, 0 failed, 152 ignored** across targets;
A historical external run recorded **2 passing** explicit pinned-source contract tests.
The independent reviewer rechecked the publishing correction and approved the
scoped commit. Live frontend/deployment acceptance remains with the parent.

## Live verification — 2026-09-11 UTC

- Deployed source `b2937cf` after combined isolated-worker formatting, ordinary tests, strict
  Clippy, workers, startup and the focused API HTTP fixture passed. See the
  [combined deployment record](repair-remote-reply-thread-persistence.md#combined-isolated-worker-validation-and-live-recovery--2026-09-11-utc).
- Through the permanent public origin, GET trends tags/links/statuses, directory,
  link timeline and instance domain blocks all return **200 `[]`**.
- Unauthenticated suggestions, familiar-followers and owner domain-block reads
  return **401**, not placeholder 404 or fabricated authenticated success.
  Authenticated empty shapes, scopes and actual moderation data were verified by
  the isolated HTTP fixture; no live token was obtained for this check.
- Unknown API route and a genuinely absent status both retain **404**. No live
  mutation was attempted; unchanged mutation errors are fixture-covered.
- Bundled public profile/status pages loaded without browser runtime errors.
  The historical external artifacts are not in the repository. Scoped
  collection-read acceptance is satisfied; issue archived. The explicitly
  deferred noncollection/lookup routes above remain unimplemented; this is not
  a claim that every possible API 404 is gone.
