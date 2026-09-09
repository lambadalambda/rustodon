# Implement v1 policy and abuse controls

## Summary

Centralize authorization, moderation-state, and abuse limits across protocols.

## Requirements

- Enforce account/user/global-domain blocks, mutes, suspended/silenced/deleted/
  moved state, and local role permissions consistently.
- Add bounded login, inbox, signature, delivery, and remote-fetch limits.

## Acceptance Criteria

- Cross-surface policy matrices prove no private/direct leaks and no action can
  bypass persisted moderation state.

## Notes

- Centralized status audience authorization and context suppression in a pure,
  typed policy module. Root status loading cannot bypass authorization, quotes
  reuse the same fail-closed decision, and unknown visibility is denied before
  owner exemptions.
- Pure and restored-database matrices cover public, unlisted, private, direct,
  limited, deleted, suspended, blocked, domain-blocked, silenced, muted, and
  unknown-visibility behavior. Rails-versus-Rust status authorization remains
  differential-compatible with Mastodon 4.6.5.
- Added all 23 typed Mastodon 4.6.5 local-role permissions, exact EVERYONE-role
  inheritance, direct-administrator expansion, any-of checks, strict hierarchy,
  and typed block-bypass decisions. OAuth credential and restricted-feed policy
  use the exact token owner user/account pair; crossed identities fail closed.
- Startup and cutover preflight reject a missing mandatory EVERYONE role rather
  than allowing role behavior to diverge by query.
- Added account lifecycle policy for active, limited, moved, memorial,
  temporarily suspended, and permanently unavailable accounts. Browser
  authentication and functional API access are separate decisions, and OAuth
  uses deletion-request state to preserve Mastodon's suspension semantics.
- Added global-domain policy with Mastodon-compatible transitional IDNA,
  normalized longest-parent matching, silence/suspend/noop behavior, media and
  fail-closed unknown or NULL severity handling. Domain block updates also
  repair nullable or unknown existing rules without weakening them. Local
  report creation follows Mastodon's `ReportService`; `reject_reports` remains
  an inbound remote `Flag` concern. Transport call-site enforcement remains
  with the inbox/fetch/delivery issues.
- Added browser login throttles matching Mastodon 4.6.5: 25 attempts per
  client IP in five minutes and 25 attempts per normalized email in one hour.
  The limiter runs after CSRF and input validation but before credential lookup;
  production state is shared through PostgreSQL and local fallback fails closed
  if its mutex is unavailable.
- Added the Mastodon 5-per-10-minute client-IP throttle for OAuth application
  registration. Production attempt windows use shared PostgreSQL state, while
  local fallback windows use fixed epoch buckets, bounded state, and
  Mastodon-compatible reset headers on throttled responses.
- Added action-specific favourite authorization: a user cannot create a
  favourite for an author they have blocked, even when the write repository is
  called below the REST read-authorization boundary. The guarded schema test
  proves a not-found result with no favourite row or counter change.
- Extended action-specific authorization to reblogs and replies: blocked
  authors and explicit/private audiences are rejected before reblog rows,
  counters, or delivery are created, and reply targets reuse the typed status
  access policy. Restored-fixture tests cover blocked reblogs and inaccessible
  replies.
- Inbound remote `Flag` reports now resolve through the durable inbox worker,
  preserve bounded comments and same-domain report URIs, persist local target
  status and collection IDs with Mastodon's remote visibility and mention
  rules, resolve generated and OStatus tag-style object IDs, create one report
  per local target account, notify report staff, ignore suspended reporters, and honor the
  longest matching `reject_reports` domain rule under the canonical
  account/domain locks. Local report creation does not apply that target-domain
  rule, matching Mastodon's `ReportService`; remote-target reply-reporting
  remains outside this bounded local-target slice.
- Account timeline reads now preserve an account owner's own reblogs even when
  the owner blocks, mutes, or domain-blocks the reblog source, matching Rails'
  `AccountStatusesFilter` owner path. Restored schema coverage proves the
  owner-visible boost behavior.
- Hashtag timelines retain all tagged public languages for authenticated viewers,
  matching Rails' `TagFeed#get` override, which does not call `PublicFeed`'s
  chosen-language scope. Restored schema and differential coverage prove the
  French tagged status remains visible to both anonymous and authenticated
  viewers.
- Added a bounded five-minute per-client-IP circuit around remote signature-key
  refreshes, coordinated through the shared PostgreSQL rate-limit table when a
  web pool is configured and retaining the process-local fallback otherwise.
  Only transport, DNS, no-address, client-build, and response-body failures
  trip it; cached-key verification and ordinary remote HTTP errors do not.
  Signature-sensitive ActivityPub status responses now vary on `Authorization`
  and `Signature` and become private/no-store when either is present. Actor,
  Note, activity, outbox, and follower/following collection routes now enforce
  the same optional-or-limited signature policy, with the instance actor
  remaining public; the guarded authorized-fetch route case passes. Local Note
  and outbox reads also reuse the centralized status audience policy for signed
  or OAuth viewers, including private/direct/limited access, author blocks, and
  domain blocks.
- Closed the delayed-notification suspension race at the durable creation
  boundary. Queued ordinary social activity from a suspended sender is dropped
  after resolution, while moderation and account-lifecycle notifications are
  preserved. Worker coverage proves a mention queued before suspension leaves
  neither a notification nor a notification request.
- Added a shared PostgreSQL equivalent of Mastodon's `throttle_media_proxy`: 30
  requests per trusted client IP in 10 minutes, with standard `429` rate-limit
  headers. The limiter runs before media lookup or remote fetching, uses
  namespaced keys, and coordinates independent web pools.
- Added a shared PostgreSQL ActivityPub inbox limiter: 300 requests per trusted
  client IP in five minutes, with standard `429` rate-limit headers. It runs
  before body buffering and signature-key resolution, masks IPv6 clients to
   their `/64`, and coordinates independent web pools. The signature refresh
   circuit and remote-fetch budget now coordinate through shared operational
   PostgreSQL state, with a process-local fallback when no operational pool is
   configured.
  - Added a 30-second deadline around bounded request-body reads, plus a
  two-in-flight-per-canonical-host budget for remote GET, POST, redirect-hop,
  and target-validation work. Durable expiring leases coordinate independent
  web/worker pools, while budget contention retries without recording
   remote-domain health failures or opening signature-fetch circuits. DNS answer
   sets are capped before address policy validation. Broader differential
   transport coverage remains outside this slice.
 - Outbound delivery now matches Mastodon's authorization-failure lifecycle:
   `401` remains retryable for active or temporarily suspended source accounts,
   but becomes permanent when the source is suspended without an account
   deletion request. Unit coverage proves both branches.
- Operational-schema upgrades now grant only newly introduced runtime-table
  privileges to an existing runtime role before strict validation, and preflight
  validates a provisioned operational schema while still allowing the initial
  pre-migration run. Cross-version and revoked-privilege fixture checks pass.
- Large bodies on required API routes are now admitted only after valid bearer
  header or query credentials are checked. Requests without those credentials
  and public routes use the 4 MiB Rack parameter ceiling instead of reserving
  the 99 MiB REST buffer before authentication; browser profile uploads require
  a valid session before retaining their 12 MiB route limit. Body-supplied
  bearer credentials remain limited to the public cap because they cannot be
  validated without reading the body.
- Account status filtering now matches Rails' `reblogs_may_occur?` rule: tagged
  and media-filtered account status requests retain reblogs even when their
  source account is blocked, muted, or domain-blocked, while ordinary pages
  still suppress those sources. Restored-fixture coverage proves tagged
  reblog retention.
- ActivityPub status documents retain browser/OAuth viewer behavior, while
  outbox and replies/likes/shares collections now use signed-request identity
  only, matching Rails' route-specific controllers. Differential and browser
  authentication coverage proves private collections reject cookie and bearer
  viewers; signed collection responses are private and not stored.
- Added the Rails `throttle_api_media` equivalent for authenticated media POSTs:
  30 requests per user in 30 minutes across both `/api/v1/media` and
  `/api/v2/media`. The user bucket is identified before scope authorization so
  insufficient-scope bearer requests count like Rack::Attack. Production web
  state stores the fixed window transactionally in PostgreSQL, so independent
  pools share the bucket and database failures fail closed; unit, cross-pool,
  and differential coverage prove the 31st request is rate limited.
- Hardened shared-cache isolation for viewer-sensitive responses. ActivityPub
  status entries vary on `Authorization` and `Signature` even when the origin
  response is anonymous and cacheable; requests carrying viewer credentials
  become `private, no-store`. Status-authorized Paperclip media varies on the
  same credentials, keeps anonymous public caching only for public variants,
  and makes authenticated local/cached-remote media private and non-storable.
- Remote media finalization now evaluates the typed `reject_media` policy in its
  write transaction while the dedicated advisory-lock transaction remains held,
  rather than relying only on a general-pool check immediately before that
  transaction. Restored worker coverage proves a blocked remote media attachment
  is failed without changing its parent status; the case runs through the
  restricted writer with limited federation enabled.
- Browser authentication now keeps Rails' authentication and functional-access
  decisions separate: disabled, suspended, and moved non-memorial users may
  authenticate and retain a browser session, while memorial users remain
  rejected. The browser differential case covers the lifecycle boundary;
  functional API access still fails closed through the OAuth policy, and the
  browser OAuth-consent route rejects non-functional sessions before grant
  creation.
  - The status authorization cross-surface acceptance matrix now passes against
  Mastodon 4.6.5, including REST, timelines, collections, notifications,
  conversations, media, and streaming suppression. This issue remains open for
  the broader abuse-control and live peer/convergence work.
  - CORS now covers the pinned Rails OAuth token, revoke, and userinfo endpoints
    plus `/.well-known/*`, `/nodeinfo/*`, `/@:username`, and `/users/:username`
    discovery/account responses. Supported preflights remain route-aware and do
    not broaden unknown API paths.
  - Resolved the final remote `Flag` review follow-ups: numeric
    `/collections/:id` web URLs resolve without a stored URI, and worker-fixture
    teardown removes reports, collection links, notifications, notification and
    mail jobs, durable jobs, and immutable stream-event outbox rows for accepted,
    suspended, and rejected activity URIs.
