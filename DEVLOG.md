# Development Log

## 2026-09-09

- Closed an authenticated browser-login open-redirect edge case: `return_to`
  values beginning with `/\\` could be normalized by browsers as an external
  authority. The validator now rejects backslashes, with a regression test;
  `mise run check` passes.
- Added restored-fixture coverage for ambiguous remote-media metadata commits.
  A test-support fault now exercises both a rollback before `COMMIT` and an
  error after PostgreSQL commits; staged Paperclip files remain available for
  retry reconciliation, and the durable jobs finish without leaked leases.
  `mise run worker-integration` passes 45/45.
- Added an eight-wave, twenty-user durable-worker executor soak with four remote HTTP
  permits, retry-after-commit idempotency, exact-key, dead-letter, and queue
  drain assertions. End-to-end production load remains open by design.

## 2026-09-07

- Completed the browser logout surface: every Rust-owned settings page now
  renders a CSRF-protected accessible form, HTML logout redirects to sign-in,
  and JSON logout returns Mastodon's `redirect_to` response while deleting the
  browser session and its OAuth token. The browser authentication and account
  settings differential cases pass against the pinned Mastodon 4.6.5 fixture.
- Aligned inbound remote Note interaction counts with Mastodon: ActivityStreams
  `likes`/`shares` collections and legacy count fields are stored as bounded
  untrusted values in the `0..100_000_000` range. Unit coverage includes
  negative and oversized counts, and restored worker integration remains 43/43.
- Matched Mastodon's case-insensitive HTTP(S) WebFinger resource parsing. The
  guarded federation-discovery differential case now covers a mixed-case URL
  scheme and passes against the pinned 4.6.5 fixture.
- Preserved stored media `blurhash` values in outbound ActivityPub Note
  attachments. A focused serializer regression and the guarded federation
  discovery differential case pass.
- Preserved Mastodon's original media `width` and `height` fields in outbound
  ActivityPub Note attachments, while omitting missing or malformed dimensions.
  Serializer coverage and the guarded federation discovery differential case
  pass.
- Outbound ActivityPub Note attachments now expose materialized thumbnails as
  Mastodon-compatible `Image` icons, including cache-aware Paperclip URLs;
  remote thumbnail URLs without a stored thumbnail file remain omitted.
- Outbound ActivityPub Notes now serialize Mastodon's automatic quote
  `interactionPolicy`, mapping public, followers, and following approval bits
  to their ActivityStreams collections and falling back to the author actor
  when no recognized bit is enabled. Unit and federation differential coverage
  pass.
- Accepted remote quote rows now expose Mastodon's `quoteAuthorization` URI on
  outbound web and durable-worker Notes. Pending, rejected, and missing approvals
  remain omitted. The quoted fixture differential case and worker integration
  pass.
- Added Mastodon-compatible local `QuoteAuthorization` ActivityPub documents at
  both username and numeric account routes. Accepted quote state, target-status
  visibility, and deleted-object checks are enforced before serialization; the
  guarded federation differential case covers both routes.
- Preserved valid media focus metadata as ActivityPub `focalPoint` lists while
  omitting incomplete or malformed focus values. Serializer coverage covers
  both branches.
- Matched Mastodon's account-level sensitivity behavior for outbound Notes:
  sensitized authors now mark their Notes sensitive even when the status itself
  is not flagged.
- Preserved Mastodon's `_misskey_quote` alias alongside `quote` and `quoteUri`
  for outbound quoted Notes; the quote serializer regression now covers all
  three identifiers.
- Added deterministic simulated quota-failure coverage for Paperclip media.
  A test-support storage-full fault after the original file write proves that
  derivative failure removes every partial file, preserves the unmaterialized
  database row, and allows a durable remote-media retry to succeed. The focused
  Paperclip test and restored worker integration pass; real filesystem quota
  exhaustion remains external hardening evidence.
- Closed the final inbound remote `Flag` fixture gaps: local collection
  resolution now accepts Mastodon's numeric `/collections/:id` web form, and
  teardown removes report-linked notifications, notification jobs, mail jobs,
  durable jobs, and immutable stream-event outbox rows for every test Flag URI.
  The restored worker fixture passes 43/43 and the full `mise run check` gate is
  green; separate `LOCAL_DOMAIN`/`WEB_DOMAIN` route matching remains a broader
  parity follow-up.
- Hardened signed ActivityPub POST transport with deterministic fixtures for
  mixed DNS answers, redirect-hop rebinding, same-origin 307/308 follow-up,
  302 rejection, timeout, and oversized responses. The full `mise run check`
  gate is green, including Clippy, formatting, all-target tests, dependency
  policy, and pinned-fixture verification; live Mastodon peer convergence
  remains external acceptance work.
- Extended the restored ActivityPub relationship worker matrix with duplicate
  Accept, Reject, Block, and Undo Block deliveries. The database relationship
  state converges without duplicate rows after replay, and
  `mise run worker-integration` passes 43/43; live peer and cross-instance
  ordering evidence remain open.
- Added a 32-address ceiling to remote DNS answer collection before SSRF
  policy validation, with a boundary unit test. This closes the remaining local
  resolver-cardinality hardening gap without changing the public address policy.
- Matched Mastodon's outbound authorization-failure behavior: a `401` delivery
  response is permanent only for a source account suspended without an account
  deletion request; active and temporarily suspended accounts retry. The
  delivery classifier regression covers both branches.
- Re-ran the complete local acceptance gates: differential compatibility passed
  20 phases, Mastodon schema passed 35/35, operational schema and streaming,
  startup safety, preflight, cutover/rollback, worker integration 43/43, and
  the full `mise run check` all passed. Live peer, browser/mobile recording,
  quota, sustained-load, and hard-power-loss evidence remain open.

## 2026-09-06

- Extended v1 hardening with a restored PostgreSQL acknowledgement-failure
  regression, a twenty-job/four-permit idempotency burst, and Paperclip
  derivative-failure rollback coverage. Worker integration now passes 42/42;
  quota exhaustion, sustained load, hard power-loss, and production reopen
  evidence remain external or future hardening work.
- Added relationship-specific outbound ordering coverage. A real signed Follow
   delivery is held open while the write path records its Undo successor; a
   second Push worker cannot claim the successor, and the released fixture sees
   Follow before Undo. Live peer convergence remains open.
- Added the matching live-lease Block/Undo ordering regression. The restored
   worker fixture holds a signed Block delivery open, records its Undo successor,
   fences a competing Push claim, and verifies Block before Undo on the wire.
   Worker integration now passes 43/43; live peer convergence remains open.
- Corrected suspension-origin handling against Mastodon 4.6.5: local moderation
  now has restored-fixture proof for remote-follow teardown, notification and
  counter cleanup, pending Accept cancellation, and durable Reject intent, while
  remote-origin actor `Update` suspension remains non-destructive to follows.
- Aligned outbound Reject identity with Mastodon 4.6.5. Persisted Follow and
  FollowRequest rows now use their numeric row IDs across blocking, follower
  removal, request rejection, actor deletion, and suspension; an immediate
  blocked Follow keeps Mastodon's empty no-row suffix. Unit, schema, and worker
  coverage pass, while live peer convergence remains open.
- Closed the basic moderation and reconciliation issue after the restored
   moderation lifecycle, domain purge, counter repair, rollback, differential,
   and suspension-side-effect gates passed. Live peer and production evidence
   remain tracked by the broader acceptance and hardening issues.
- Extended inbound remote `Flag` reports to retain local target status IDs and
   collection relationships, matching the pinned Mastodon 4.6.5 ActivityPub
   handler's visibility and mention rules for local targets. One Flag now
   creates a report per local target account, with canonical account/domain
   locks around suspension and `reject_reports` policy checks; generated and
   OStatus tag-style object IDs are resolved while unsupported report IDs are
   filtered. The
   restored worker fixture proves account, public/private/direct status,
   collection, multi-target, suspended-reporter, and domain-rejection
   behavior; remote-target reply reporting remains outside this local-target
   slice.
- Closed the status-social-interactions issue after rerunning the current gates:
  restored Mastodon schema integration passes 35/35, worker integration passes
  43/43, and the full differential workflow passes 18 general cases plus the
  notification and status-authorization phases. Interaction routes, counters,
  idempotency, locking, notification, and outbound lifecycle behavior are now
  locally evidenced; live peer convergence remains external.
- Closed the account-relationship-writes issue after the same current gates
  proved duplicate-safe follow, follow-request, block, mute, counter,
  notification, expiry, and local outbound-intent behavior. Remote peer
  convergence remains with the ActivityPub federation issues.
- Closed the write-foundation issue after the current differential write matrix,
  restored 35/35 Mastodon schema tests, cutover reopen rehearsal, and narrow
  typed writer boundary verified the transaction, locking, idempotency, outbox,
  and database/media contracts. Feature-specific and live-peer work remains
  with its owning issues.
- The complete Rails-versus-Rust differential workflow passes 18 general cases
  plus isolated notification-write and status-authorization phases (20 test
  phases total) against Mastodon 4.6.5. The earlier ten-minute wrapper timeout
  was command-duration related; `rest_protocol_contracts` passes in isolation
  and the full workflow completes with a longer timeout.
- Fixed cutover web-process cleanup by making `run_web_cli()` replace its
  intermediate shell with Rustodon via `exec`; rerunning
  `mise run cutover-integration` passed and left no Rustodon web listeners
  after cleanup. Stale listeners from earlier rehearsals were removed.
- Completed the browser TOTP/recovery-code management slice. Rust-owned setup,
  confirmation, regeneration, and disable routes now use Active Record encrypted
  OTP secrets, bcrypt recovery-code hashes, password challenges, required-role
  protection, and transactional WebAuthn cleanup. Guarded Mastodon 4.6.5 cases
  prove the encrypted production repository path, rate-limit boundary, role
  protection, recovery lifecycle, and database restoration; live browser/client
  evidence remains open.
- Added representative Mastodon-owned preservation rows for an unresolved
  account migration, announcements, appeals, backups, a failed bulk-import row,
  report notes, and a valid Web Push subscription. SQL and Rails verification
  assert their values and foreign-key links; the 35-case schema gate, complete
  19-case differential suite, and cutover/rollback rehearsal all prove the rows
  survive Rustodon startup and Mastodon reopen. `PRESERVE-06` is now locally
  automated; production and client acceptance remain open.
- Extended the cutover rehearsal with a fresh JPEG upload through Rustodon. The
  isolated media root is reopened by pinned Mastodon 4.6.5, where Rails metadata,
  the v1 media response, and original/small Paperclip files are verified before
  restoring the database row, sequence, and filesystem baseline. The old Redis
  container is removed before Mastodon is restored with a fresh empty Redis
  instance. Paperclip file creation now explicitly applies `0644` after
  restrictive process umasks so a reopened Mastodon process running as another
  user can read new media. The cutover gate passes.
- Extended the browser account-settings differential case with an isolated
  moderator profile upload. Multipart avatar and header submissions now prove
  redirect behavior, descriptions, JPEG metadata, generated Paperclip files,
  and database/media cleanup back to the baseline. The focused case passes
  against the restored Mastodon 4.6.5 fixture; client/mobile acceptance remains
  open.
- Extended the authenticated WebSocket fixture with deterministic reconnect
  coverage. After the initial batch is consumed, a post-handshake event is
  delivered exactly once on the new connection and earlier events are not
  replayed. Operational-schema integration passes; full pinned-client filtering
  and live peer convergence remain open.
- Fixed a profile-settings media regression: saving profile fields without an
  avatar or header upload no longer deletes the existing Paperclip files. The
  account-settings differential case now snapshots the Rust media tree to keep
  this invariant covered. The complete Rails-versus-Rust workflow passes 19/19
  cases against Mastodon 4.6.5 (17 general plus notification and status phases).

## 2026-09-03

- Completed the account-lifecycle write-fencing pass. Authenticated local write
  entry points now recheck lifecycle state inside their transactions through the
  shared `begin_account_write` helper; profile/media filesystem work remains
  under the canonical account lock. Actor-delete delivery rechecks state while
  holding that lock through the final send, and account purge repairs accepted
  quote counters when source statuses disappear. The restored worker gate passes
  39/39, the Mastodon schema gate passes 35/35, and `mise run check` is green.
  The stale-authentication regression now covers every identified authenticated
  write entry point; hard power-loss compensation remains open.
- Advanced v1 release hardening with a deterministic all-lane worker crash
  regression. An aborted leased handler is reclaimed and acknowledged exactly
  once across Ingress, Core, Push, Pull, Mail, and Maintenance. Worker
  integration now passes 40/40; disk-full, PostgreSQL outage, sustained-load,
  power-loss, and live-peer reopen evidence remain open.
- Added the executable `mise run cutover-integration` rehearsal. It migrates
  Rustodon's isolated operational schema, starts the least-privilege worker and
  web processes, runs health/auth/read/write/media/WebFinger/ActivityPub smoke,
  stops Rustodon, reopens the pinned Mastodon web process, verifies Rails
  reads, and proves stable public catalog/schema/data/auth state and Paperclip
  media are preserved without reversing Mastodon migrations. The gate passes.
- Hardened `tools/rustodon-smoke` for local reverse-proxy fixtures: it sends the
  configured host, uses native HEAD requests, forwards optional trusted HTTPS
  protocol metadata, forwards public-request curl options, and keeps bearer
  tokens out of curl process arguments via a temporary mode-restricted header
  file.
- Extended the restored moderation worker proof for administrative domain purge:
  case-normalized exact-domain selection preserves subdomains, removes remote
  account/status/report/notification descendants and Paperclip files, refreshes
  instances, preserves local counters and severance type boundaries, emits no
  stream effects, and safely replays after completion. Worker integration passes
  34/34; crash-time filesystem compensation and live peer evidence remain open.
- Added a restored-fixture remote-media lease-fence regression: a blocked fetch is
  fenced after its durable lease expires, leaves the attachment and job
  recoverable, and a recovery invocation materializes the original and small
  derivatives without a dead letter. Worker integration passes 35/35; concurrent
  stale writes, ambiguous metadata commits, and shutdown-specific media
  cancellation remain open.
- Added route-aware CORS preflight and response headers for OAuth token/revoke,
  userinfo, discovery, NodeInfo, and public account endpoints. OAuth browser
  sign-in now preserves a validated local authorization return target, and the
  authorization-code differential case plus focused browser tests pass.
- Replaced the process-local remote signature-refresh cooldown with a shared
  PostgreSQL marker in `rustodon.rate_limit_windows` when a web pool is
  configured, retaining the local fallback for pool-less state. Transport/DNS/
  client/body-read failures still trip the five-minute marker, while ordinary
  remote HTTP errors do not. Independent-pool visibility, client isolation, and
  expiry cleanup pass in the operational fixture.
- Added expired `rate_limit_windows` pruning to the durable maintenance handler;
  the least-privileged runtime worker integration now proves marker cleanup
  alongside the existing operational maintenance checks.
- Post-CORS and shared-circuit verification passes `mise run check` (193 passed,
  2 ignored), `mise run operational-schema-integration`, formatting, diff, and
  Clippy checks. The direct `cargo deny` command is unavailable outside Mise;
  Mise's pinned dependency-audit task passes.
- Added operational-schema migration 3 with expiring per-canonical-host remote
  fetch leases. Web and worker fetchers now coordinate the two-request budget
  across independent pools through transaction-locked PostgreSQL rows, release
  leases on success/error/cancellation, reclaim expired rows during acquisition
  and maintenance, and retain the process-local fallback for pool-less tests.
- Added least-privilege runtime grants, schema catalog/upgrade coverage, and
  independent-pool lease tests. The local gate passes 193 tests with two ignored,
  operational-schema integration passes, worker integration passes 34/34, and
  startup integration passes 4/4 against Mastodon 4.6.5/PostgreSQL 14.23.
- Added transactional grants for newly introduced operational tables during
  upgrades, so an existing v2 runtime role can migrate to schema v3 without
  failing before the new remote-fetch lease privilege is available. The
  fixture-backed v2-to-v3 upgrade regression passes.
- Preflight now validates an existing Rustodon operational schema and runtime
  role, while allowing the documented first run before operational migration.
  The preflight fixture proves both the valid path and a revoked lease grant.
- Added `docs/mastodon-writer-grants.sql`, an executable least-privilege writer
  ACL recipe covering Mastodon and Rustodon tables, columns, sequences,
  functions, and rejected `PUBLIC` grants. The restored startup fixture now
  executes the same recipe.

## 2026-09-02

- Completed the least-privilege writer ACL review for remote media, quotes,
  tombstones, and account-deletion request row locks. Preflight now requires
  `domain_allows`, `quotes`, tombstone table/sequence capabilities, and
  `UPDATE` on `account_deletion_requests` for its `FOR UPDATE` path; the
  restored fixture grants exactly those capabilities. Startup coverage rejects
  revoked tombstone access, an unexpected quote insert grant, and arbitrary
  column-level `SELECT` or `REFERENCES` grants.
- Re-ran the restored-fixture gates after the ACL remediation: schema 35/35,
  worker 34/34, startup 4/4, and all 19 Rails-versus-Rust differential cases
  passed against Mastodon 4.6.5 (17 general plus notification and status
  authorization phases) in 724.31 seconds.
- Extended the restored WebSocket fixture coverage so an unauthorized private
  status update is suppressed before an authorized public update is delivered.
  Operational schema integration passes, including both streaming tests.
- Isolated stateful differential phases by restarting Rails/Redis and
  re-precomputing feeds before notification and status authorization cases.
  The complete differential workflow passes 17 general cases plus the two
  isolated cases.
- Completed the PostgreSQL 14 writer-ACL hardening. Startup validation now
  checks default `CREATE` privileges, explicit public ACL drift on types,
  languages, foreign objects, and tablespaces, large-object settings and ACLs,
  and ownership across the supported PostgreSQL catalog object classes while
  preserving PostgreSQL's built-in default ACLs.
- Expanded guarded startup mutations for default schema privileges, public
  foreign-object and tablespace grants, catalog objects, large objects, and
  role settings. The restored production startup safety integration passes all
  3/3 tests against the pinned Mastodon 4.6.5/PostgreSQL 14.23 fixture, and
  `mise run check` passes with 191 unit tests and one ignored test.

## 2026-09-01

- Completed the read-only writer-pool and advisory-lock review against the pinned
  Mastodon 4.6.5 checkout. Found shared-pool self-starvation when lock callbacks
  acquire nested connections, a web/admin `DB_POOL` sizing mismatch, and a
  single-label domain-block lock-scope gap. Startup, worker, and preflight
  integration gates pass after restoring the fixture function grant in the
  unsafe-role test; remediation remains tracked in the open review issue.
- Remediated the reviewed resource and race findings: lock ownership uses a
  dedicated PostgreSQL connection, configured writer pool sizes reach web and
  admin constructors, remote actor upserts recheck policy inside their lock,
  and remote media retries can reclaim stale `processing = 1` claims.
- Remote media cleanup now removes staged files on cancellation before commit,
  retains files across ambiguous commits for retry reconciliation, and refuses
  stale error paths from downgrading rows that already have a file. The seeded
  stale-claim worker regression and the full restored worker suite pass 34/34.
- Remote media finalization now evaluates the typed global-domain `reject_media`
  policy in its write transaction while the dedicated advisory-lock transaction
  remains held. A restored worker regression proves a domain-blocked attachment
  is failed closed without changing its parent status.
- A fresh full Rails-versus-Rust differential run passed all 19 cases against
  Mastodon 4.6.5 in 681.25 seconds. The earlier ten-minute wrapper timeout was
  isolated to setup/runtime duration; the individual OAuth case also passed.
- Final verification passes the 192 library tests (191 passed, 1 ignored), all
  non-ignored integration targets, lint, dependency audit, startup 3/3, schema
  35/35, preflight, worker 34/34, operational-schema, formatting, and diff
  checks. Actual lease-fence and ambiguous-commit fault injection remain open.
- Favourite and bookmark removal now follows Rails' association-first behavior
  after an author block, including the unauthorized response projection. Remote
  favourite deletion is no longer blocked by creation-time domain policy, so
  existing rows can be removed and Undo delivery recorded. The guarded
  interaction differential and all 35 restored-fixture schema tests pass.
- Global domain-block side effects now cross a durable boundary. Suspend-level
  and reject-media updates enqueue a transactional maintenance job; the worker
  rechecks the block generation and account suspension timestamp before applying
  cleanup, so retries cannot purge a newly changed account state.
- Domain suspension now records Rails-compatible relationship severance events,
  preserves active/passive follow settings, creates local severance notifications
  idempotently, clears matching Paperclip metadata, removes configured files
  without following symlinks, and deletes remote custom emoji rows. Restored
  Mastodon schema and worker coverage pass, including a filesystem-backed case.
- Remote actor `Update` activities now validate and persist Mastodon's
  `suspended` state with remote suspension origin, allow remote unsuspension,
  and keep local suspensions and deleted-account tombstones fenced. Remote Note
  and actor deletion plus due account purge now collect Paperclip metadata before
  safely removing files; reported account content remains protected. The full
  worker integration suite passes 32/32.
- Added the least-privilege writer contract for severance tables and sequences,
  and removed row locks from the `poll_votes` read that the writer role cannot
  legally acquire. The remaining gaps are full admin domain purge semantics,
  ancillary remote-suspension side effects, immediate streaming disconnects,
  and crash-time filesystem/database compensation.
- Writer preflight now permits exactly the severance updates and shared
  `rate_limit_windows` operations used by the web writer pool; the fixture ACL
  contract and production startup safety integration pass. Remote media fetch
  completion also rechecks its parent status is still live before committing
  metadata, cleaning staged files when a concurrent deletion wins.
- ActivityPub inbox ordering and logical activity identity now use the verified
  actor URI rather than a signing key ID. Retained fingerprints reject
  conflicting retries with `409 Conflict`; a restored-fixture HTTP regression
  proves key-rotation ordering, idempotent retries, and conflict rejection.
- Explicitly ran the full write-transaction differential case after the
  association-first saved-interaction changes; the blocked bookmark/favourite
  removal path passes against Mastodon 4.6.5. The restored durable-worker suite
  passes 33/33, including signed delivery, transient retry, crash replay, and
  same-inbox cross-worker ordering for outbound ActivityPub work. Live peer
  convergence remains the only unverified delivery boundary.
- Embedded remote `Undo Follow` and `Undo Block` writes now fence deletion by
  the referenced relationship URI, preventing an older undo from removing a
  newer relationship for the same actor pair. Restored-fixture coverage proves
  stale and matching embedded undos for both relationship types.
- Local suspension and self-service account deletion now record an atomic
  `kill` stream event. WebSocket connections poll system events without waiting
  for a subscription and close with Mastodon's normal code; a second
  post-cursor authentication closes the suspension race. The operational fixture
  proves the live suspension path and preserves public schema/data immutability.

## 2026-08-31

- Added the Rails `throttle_api_media` equivalent for authenticated `POST`
  requests to both media API versions: 30 requests per user in 30 minutes.
  User identity is resolved without requiring write scope before the normal
  media authorization path, matching Rack::Attack's treatment of insufficient-
  scope bearer tokens. Production web state uses a transactional PostgreSQL
  window, so independent pools share the bucket and database failures fail
  closed. Unit, cross-pool, and restored differential media coverage prove the
  boundary and rate-limited response behavior without leaving media artifacts.
- Hardened credential-sensitive cache boundaries. ActivityPub status responses
  now vary on `Authorization` and `Signature` even for anonymous cache entries,
  and requests carrying viewer credentials receive `private, no-store` instead
  of entering a shared cache. Status-authorized Paperclip media keeps public
  anonymous caching only with credential-aware variation; authenticated local
  media and cached remote media are private/no-store. Unit, federation, local
  Paperclip, and discarded-media differential coverage prove the policy.
- Separated browser authentication from functional account access to match the
  pinned Rails lifecycle. Disabled, suspended, and moved non-memorial accounts
  can authenticate and retain a usable Rust-owned browser session; memorial
  accounts remain rejected. The differential browser-authentication case covers
  all four lifecycle states. Browser sessions now carry functional state so
  non-functional accounts cannot reach OAuth consent, while OAuth/API
  authorization remains fail-closed.
- Administrative local suspensions now enqueue the existing durable account
  purge job for 30 days after the deletion request, transactionally with the
  moderation state, plus a same-time durable ActivityPub actor-delete intent.
  Restored Mastodon schema coverage verifies both scheduled payloads and that
  unsuspension cancels them.
- Added the `rate_limit_windows` CRUD privileges to the operational-schema
  runtime ACL allowlist. The worker integration now migrates twice and passes
  all 30 durable-worker cases with the least-privilege role.

## 2026-08-29

- Account status pages now match Rails' `reblogs_may_occur?` behavior: tagged
  and media-filtered requests do not apply source-account block, mute, or
  domain-block filters. Restored-fixture coverage proves a tagged reblog is
  retained when its source is blocked.
- ActivityPub viewer authentication is now route-specific: status documents
  retain browser/OAuth access, while outbox and replies/likes/shares collections
  use signed-request identity only. Private collection responses reject browser
  and bearer viewers, and signed responses are marked private/no-store.
- Follow-request authorization now emits an ActivityPub `Accept` only for
  remote accounts whose protocol is ActivityPub, matching Rails' source-account
  check. OStatus requests still become local follows without outbound Accept
  delivery; restored-fixture coverage proves both paths.
- ActivityPub inbox requests now have a process-local 300-per-five-minute
  trusted-client-IP limit before body buffering or signature-key resolution.
  IPv6 clients share their `/64` bucket and the limiter returns Mastodon-style
  rate-limit headers; the boundary is covered by a unit regression.
- Bounded request bodies now have a 30-second read deadline in addition to
  their byte limits, preventing stalled clients from holding REST or inbox
  workers indefinitely. A shared process-local remote-fetch budget also caps
  each canonical remote host at two simultaneous GET/POST/DNS operations;
  contention retries without tripping signature circuits or domain health, and
  idle buckets are evicted at the bounded host-state ceiling. SSRF validation
  also normalizes IPv4-compatible IPv6 addresses through the IPv4 policy.
- Large bodies on required API routes are now admitted only after valid bearer
  header or query credentials are checked. Requests without those credentials
  and public routes use the 4 MiB Rack parameter ceiling instead of reserving
  the 99 MiB REST buffer before authentication; browser profile uploads require
  a valid session before retaining their 12 MiB route limit.
- Account-update reach now uses Rails' `suspended_at - 2 days` cutoff for locally
  suspended accounts instead of the wall-clock cutoff. Restored worker coverage
  proves a delayed actor update still reaches a recently followed remote account.
- Status updates now apply nested Mastodon `media_attributes[]` descriptions and
  image focus values to retained attachments transactionally. The status edit
  path now creates the initial previous snapshot only once, matching Rails on
  repeated edits. Schema and differential coverage pass, including REST response
  metadata and edit-history behavior.
- Status media IDs now preserve Rails' first-occurrence order without duplicates,
  historical edit media is capped at four attachments, and edit timestamps use
  the persistence time for `updated_at`. Video thumbnail replacement remains
  outside the image-only media scope.
- Remote URI-only Accept/Reject decisions now require an exact follow URI and
  cannot consume NULL-URI relationship rows. Restored-fixture worker coverage
  proves mismatched decisions are no-ops while matching decisions remain valid;
  worker integration passes 29/29.
- Corrected global-domain severity handling: nullable and unknown existing
  rules remain fail-closed in policy and can be repaired without weakening
  them. Local report creation now matches Mastodon `ReportService` and is not
  rejected based on the target domain; `reject_reports` remains an inbound
  remote `Flag` concern, outside the current v1 scope. Schema integration
  passes 30/30.
- Added a process-local Mastodon-compatible `media_proxy` abuse throttle: 30
  requests per trusted client IP in 10 minutes, with the standard `429`
  rate-limit headers. The limiter runs before media lookup or remote fetching;
  the unit boundary test proves requests 1-30 pass and request 31 is denied.
- Status language values now follow the pinned Rails locale cascade: supported
  regional locales are preserved, unsupported variants such as `fr-FR` fall
  back to `fr`, and unknown values fall back to the current/default language.
  The guarded differential write case proves the persisted edit state.
- Notification-request merge status now reflects pending transactional unfilter
  work in either the operational outbox or durable-job queue instead of always
  returning `merged: true`. Worker integration proves outbox-pending,
  durable-job-pending, and completed states.
- Single notification-request dismissal now records a per-request transactional
  cleanup job. The Core worker deletes that sender's filtered notifications in
  bounded batches; bulk dismissal retains the pinned Rails direct-destroy
  behavior. Worker coverage proves delayed cleanup and repeated dismissals.
- Unknown ActivityPub Note Updates older than 24 hours are now ignored before
 materialization, matching the pinned Rails Update path. Known statuses and
 tombstones remain unaffected; unit and worker regressions cover the fence.
- Canonical ActivityPub Note reads and local outbox pages now apply the shared
  status authorization policy to signed/OAuth viewers. Private, direct, and
  limited content is exposed only to the correct audience, while author blocks
  and domain blocks remain fail-closed; the federation differential case covers
  anonymous, follower, mentioned, blocked, username, numeric, and paginated
  routes.
- Added username and numeric ActivityPub status collection routes for `replies`,
  `likes`, and `shares`. Standalone replies serialize local Notes but preserve
  remote URI items, follow Rails' self-reply/other-account pagination, and apply
  parent status authorization before returning any collection or count.
- Matched Rails boost routing by redirecting status-object GETs to the original
  status and keeping boost collection URLs off the `/activity` object URI. Note
  and activity responses now include the exact alternate ActivityPub `Link`
  header; the guarded federation case checks redirects, collection IDs, and
  status-document headers.
- Matched Rails status response caching while hardening shared-cache isolation:
  non-credentialed distributable status documents use `max-age=180, public`,
  pending quotes use five seconds, and every status entry also varies on
  `Authorization` and `Signature`; credentialed status responses are private
  and not stored. Differential coverage compares the Rails baseline while
  explicitly allowing these security-only cache additions.
- Applied the same Rails `Vary` and `private, no-store` defaults to status-route
  errors and local boost redirects, and extended the federation differential
  checks to cover non-200 status responses as well as successful documents.
- ActivityPub status and collection reads now honor valid browser session cookies
  before signed or bearer viewers, matching Rails web-session precedence; limited
  federation still requires a valid request signature. Optional invalid
  signatures continue to fall back to anonymous public-fetch reads.
- Added Rails-compatible `inReplyToAtomUri`, `conversation`, and `context` Note
  fields for federation reads and outbound worker Notes, including generated
  local OStatus reply identifiers when a parent has no stored URI.
- Matched Rails' five-second public cache window for distributable ActivityPub
  statuses with pending quotes. The status and activity routes query pending
  quote state before applying the shared response cache policy, while Activity
  objects retain their normal three-minute cache.
- Quote listing now filters both directions of account blocks before applying
  pagination, matching Rails and preventing hidden quote authors from producing
  stale cursors. Schema integration passes 31/31 with the regression case.
- Account-only remote ActivityPub `Flag` reports now run through the durable
  inbox worker. Comments are capped at Mastodon's 5,000-character limit,
  suspended reporters are ignored, report staff notifications and configured
  mail jobs are queued transactionally, and the longest matching domain rule
  can reject reports. Hostname policy matching now ignores remote ports while
  origin validation retains them. Retroactive account restrictions normalize
  stored port-bearing domains, and inbox actor domains strip explicit default
  ports while preserving non-default ports. Restored-fixture worker integration
  passes 30/30; status, collection, and remote-target Flag objects remain
  deferred.
- Account timeline reblog-source filtering now bypasses blocks, mutes, and
  domain blocks for the timeline owner, matching Rails' `AccountStatusesFilter`.
  Restored schema integration covers the owner-visible boost regression.
- Hashtag timelines retain all tagged public languages for authenticated viewers,
  matching Rails' `TagFeed#get` override, which does not call `PublicFeed`'s
  chosen-language scope. Restored schema and differential coverage preserve the
  anonymous-versus-authenticated result.
- Status notification fan-out now honors Mastodon's `USER_ACTIVE_DAYS` setting
  instead of hard-coding the default seven-day activity window. Missing values
  default to seven days and invalid numeric values convert to zero like Ruby's
  `to_i` boundary.

## 2026-08-28

- Reconciled outbound ActivityPub reach with the pinned Mastodon 4.6.5
  `StatusReachFinder` and `AccountReachFinder`: suspension-triggered actor
  updates are retained and delivered, same-second account updates have
  microsecond-versioned delivery keys, recent account reach caps are applied
  after preferred-inbox grouping, and alternate-host inboxes remain valid
  bounded delivery targets. Remote Likes retain direct actor inbox delivery;
  Announce and Undo Announce use the preferred shared inbox so status fan-out
  deduplication cannot discard a boost. The final local gates pass: 172
  library tests plus all target binaries, 29/29 worker tests, 30/30 schema
  tests, and all 19 differential cases. Live peer convergence, client
  acceptance, and cross-worker ordering remain open.
- Corrected outbound status reach for remote quotes by joining
  `quotes.quoted_status_id` rather than the quoting status ID. A restored
  worker regression isolates a remote quoter on a unique inbox and proves
  edited status delivery; worker integration passes 29/29. Live peer
  convergence and cross-worker ordering remain open.
- Added the executable v1 acceptance matrix and required-route inventory test,
  then ran the complete 18-case Rails-versus-Rust differential suite against
  the pinned Mastodon 4.6.5 fixture; every case passed. Browser/mobile, live
  peer, policy cross-surface, and cutover evidence remain explicitly open.
- Added a bounded process-local five-minute remote signature-key refresh circuit
  keyed by trusted client IP. Only transport/DNS/client/body-read failures trip
  it, and signature-dependent ActivityPub status responses now vary on
  `Authorization` and `Signature` with private caching for authenticated fetches.
-  Actor, Note, activity, outbox, and follower/following collection reads now
  enforce the same signature policy in limited federation mode while `/actor`
  remains public. Web and authorized-fetch coverage, federation differential
  coverage, full tests, lint, format, dependency audit, and fixture verification
  pass.
- Added the Rust-owned self-service `/settings/delete` flow with CSRF and
  password/username confirmation. A successful request atomically marks the
  local account unavailable, records Mastodon's deletion request, cancels
  pending actor updates, signs out the browser session, and queues a durable
  ActivityPub actor `Delete`. The Push worker deduplicates remote/shared
  inboxes and relays, applies federation policy, and retains the signing key
  needed by the signed delivery. Full account-content purge and relationship
  severance remain deferred. The new worker regression passes; the complete
  18-case differential suite passes.
- Browser authentication and recovery failures now negotiate HTML for browser
  form submissions instead of returning JSON into a navigation. Login errors
  preserve the submitted email and CSRF state, reset-password errors preserve
  the reset form/token, confirmation failures provide a safe return link, and
  non-HTML clients retain the existing JSON envelopes. The guarded browser
  authentication and federation cases pass against the pinned Mastodon 4.6.5
  fixture.
- Remote signed reply Creates now retain their original activity JSON and are
  durably forwarded to the local reply parent's remote followers through the
  parent account's signer, excluding the sender inbox and preferring shared
  inboxes. Forwarding keys are immutable per activity/inbox, and replaying a
 Create does not reset a dispatched delivery.
- Remote signed Note Creates now also forward through local reblogger and
  quoter followers, while signed Note Updates and Deletes retain their
  activities and use the same durable forwarding path. Restored-fixture worker
  coverage proves reply, reblog, quote, update, and delete forwarding.
- Deleting a remote original Note now records durable `Delete` intents for
  affected local reblog wrappers, allowing their `Undo Announce` activities to
  reach remote followers. Local original-status deletion uses the same
  transactional path; worker and schema integration remain green.
- Narrowed deleted-status reach to match Mastodon: `include_unsafe` still
  preserves historical interaction recipients, but deleted direct or limited
  replies no longer reach the reply target or that target's followers. A
  restored worker regression covers the privacy boundary; worker integration
  passes 29/29.
- Local reply reach now includes remote followers of the local parent author,
  with protocol, suspension, and domain-block filtering. Actor-delete coverage
  also proves deleted-actor and affected-local account counters return to the
  expected values. Worker integration passes 26/26; the aggregate local check
  passes.

## 2026-08-27

- Remote Notes addressed to specific accounts now persist with Mastodon's
  `limited` visibility (`4`) instead of being misclassified as direct; local
  direct posts remain `3`. Deleted statuses now unlink their IDs from account
  conversations, and block or notification-hiding mute writes share atomic
  cleanup for conversations, notifications, and notification requests. Focused
  coverage passes schema integration `30/30` and worker integration `26/26`.
- Added Mastodon-compatible streaming endpoint aliases for `user`,
  `user:notification`, and `direct`, including path-selected initial streams.
  Notification status updates now reach both legacy `user` and dedicated
  notification subscriptions when scopes allow it. Remote audience URIs now
  resolve through canonical local ActivityPub aliases, so silent limited
  mentions receive user-stream create/update/delete events; status deletion
  fan-out also covers all current local followers instead of reapplying live
  feed filters. Operational WebSocket, schema, and worker integration remain
  green.
- The full Rust target matrix and strict Clippy pass under the pinned Rust
  `1.97.1` toolchain. The unprefixed commands still select installed Rust
  `1.97.0`, which Cargo rejects per the repository's `rust-version` requirement.
- Completed the account, media, and authentication release-safety review. Locked
  account visibility defaults now fail private, status creation honors explicit
  and stored quote policies, and private browser posting defaults normalize quote
  policy to `nobody`. The full local check and all 18 guarded differential cases
  pass against the pinned Mastodon 4.6.5 fixture.
- Push delivery now derives a stable source-account/inbox ordering key. Pending
  outbox events dispatch in order, ordered jobs retain predecessor IDs, claims
  fence successors during live or recovering deliveries, and marker cleanup
  preserves active chains. Concurrent first-marker creation, expired-marker
  recovery, and abandoned-lease ordering now pass in the 24-test worker suite;
  live peer convergence and wire-level crash/retry proof remain open.
- Added durable-worker coverage for a real transient signed delivery retry:
  `503` records domain health and leaves the job durable, cooldown recovery
  permits the next attempt, and `202` clears both the job and failure state.
  Restored-fixture worker integration now passes 25/25; process-crash duplicate
  delivery and live peer convergence remain open.
- Added transport-boundary crash/replay coverage: a local peer accepts a signed
  POST before the worker is aborted, lease recovery replays the activity, and an
  ordered successor remains fenced until replay completes. Worker integration
  now passes 26/26; peer-side idempotency and live Mastodon convergence remain
  open.
- Corrected authenticated user-stream fan-out for limited and direct statuses.
  The pinned Mastodon behavior delivers these statuses to local followers who
  are explicitly mentioned; Rustodon now removes the contradictory public-only
  filter and proves the author/follower recipients with a restored-fixture
  regression. Schema integration passes 27/27, worker integration 26/26, and
  all 18 guarded differential cases still pass.
- Reblog create and removal now record transactional authenticated user-stream
  `update`/`delete` events. Follower fan-out honors `show_reblogs` while keeping
  the booster event and existing block/mute policy checks; the restored schema
  suite passes 28/28 and the full quality, worker, and differential gates pass.
- Stream-event writes, cursor snapshots, and reads now share a transaction-level
  PostgreSQL advisory lock, preventing pre-commit identity gaps from making a
  later committed event advance a client past an earlier one. A concurrent
  commit-order regression covers the race; operational integration passes with
  the WebSocket replay test.
- Original-status deletion now records user-stream `delete` events for every
  soft-deleted reblog wrapper before the original event. The restored schema
  regression proves wrapper cleanup for all local followers; schema integration
  now passes 29/29.
- Incoming remote Note Create/Update/Delete and Announce/Undo writes now record
  transactional authenticated user-stream `update`/`delete` events, including
  remote reblog wrappers removed by Note Delete and URI-only Undo handling.
  Restored worker coverage proves the remote lifecycle; schema integration
  passes 29/29, operational stream/WebSocket checks pass, `mise run check`
  passes 159 tests, and all 18 guarded differential cases pass. Full client
  stream filtering and live peer convergence remain open.
- Authenticated conversation events now use Mastodon's dedicated `direct`
  stream, with `read:statuses` scope enforcement and no fallback to `user`.
  User-stream status fan-out now applies the pinned home-feed language, reply,
  mute/block, domain-block, exclusive-list, mention, and reblog-author filters.
  Expiring mutes are honored consistently for reblog sources and blocked
  mentions. Mention-only status updates now emit a private internal stream
  marker for the recipient's notification stream. Restored schema and worker
  integration pass 29/29 and 26/26 respectively.
- Added the Rust-owned minimal account settings surface for profile, appearance,
  posting defaults, security, password changes, and authenticated browser session
  flows with escaped HTML and CSRF protection. Browser settings coverage proves
  persistence and rejection paths; successful browser media upload and 2FA
  management remain outside the current proof.
- Local Paperclip media and thumbnail routes now apply status audience policy
  before opening files, deny unattached/deleted media to ordinary viewers, and
  preserve Mastodon's `manage_reports` exception for discarded media. The
  authorization decision has pure unit coverage and the guarded
  `local_paperclip_deleted_media` differential case proves anonymous denial and
  moderator access after soft deletion.
- The pinned Mastodon 4.6.5 production frontend is now packaged under
  `public`, recorded with its source revision/build contract and a generated
  `SHA256SUMS` file. Rustodon serves the hashed Vite assets, PWA manifest,
  service worker, public notification assets, favicon, and SPA shell with
  escaped initial state, CSRF/VAPID metadata, authenticated session hydration,
  and deep-link fallback. The shell now covers the complete pinned web-app route
  inventory, applies Rails-equivalent HTML security headers with a nonce-backed
  CSP, rejects revoked or expired browser-session tokens, and restricts remote
  media proxy responses to Mastodon's supported media MIME types. Unit coverage
  validates the manifest and shell contract; guarded browser-auth and
  `local_web_client_shell` cases cover the live router paths.
- Completed the unknown remote Announce path through the durable Pull lane.
  Embedded self-boost Notes are persisted without a fetch; unknown targets use
  bounded same-origin-checked fetches, local-follower signatures, remote actor
  upserts, and tombstone/relevance fencing. Fetched Create wrappers preserve
  their activity URI separately from the inner Note URI, and nested Announce
  targets resolve recursively up to a bounded depth.
- Remote Note persistence now records Mastodon's ordered media attachment IDs,
  retains visibility across Update payloads, ignores content replacement when
  no explicit edit timestamp is supplied, fences media work after deletion, and
  removes dependent status/mention/quote-update notifications on delete.
- Restored-fixture worker coverage passes 22/22. `mise run check` passes with
  155 library/target tests, strict Clippy, dependency checks, and fixture
  verification. Live peer convergence and the full differential/adversarial
  federation matrix remain open.

## 2026-08-26

- Closed the remaining Rails-versus-Rust status interaction mismatches in the
  guarded fixture: duplicate and generated-ID unreblogs now preserve literal
  request semantics, remove owned boosts even when the source is no longer
  readable, maintain status/account counters, and serialize the correct
  response branch. Reblog creation now rejects both directions of blocking;
  canonical target locking follows status-deletion lock order to avoid a
  wrapper-versus-source deadlock.
- Status relationship projections now follow Mastodon's serialized-object ID
  maps, while no-op unreblog projections clear viewer relationships recursively.
  Differential setup explicitly drains Rails' asynchronous removal counters
  instead of hiding that fixture limitation in response comparisons.
- Added guarded regressions for reverse-blocked reblogs, removal after blocking
  an author, generated-ID unreblogs, proper-status favourite/bookmark flags,
  and account status-count maintenance. The full 15-case differential suite,
  26-test schema integration, and aggregate `mise run check` gate pass.
- Integrated Mastodon-compatible favourite action policy below the REST read
  boundary. Blocked authors now produce a not-found write result without a
  favourite, notification, delivery, or counter mutation; pure policy and
  restored-schema coverage prove the distinction between readable public
  statuses and actionable favourites.
- Extended action authorization to reblogs and reply targets. Reblogs now
  reject blocked authors and explicit/private audiences, while replies reuse
  the typed status-access policy before any status or counter mutation.
- Hardened durable notification creation against suspension races. Ordinary
  social notifications from suspended senders are dropped when a queued job is
  resolved, while administrative and lifecycle notifications retain their
  intended recipient semantics. Restored schema and worker coverage now pass
  26/26 and 21/21 respectively.
- Hardened known-remote ActivityPub Announce/Undo handling. Known remote
  targets now support nested boosts, unfollow relevance fencing, self-private
  remote boosts, Group notification suppression, and idempotent Undo. Remote
  Note deletion now scopes the target to its author, locks dependent boosts
  before the original status, removes their counters and notifications, and
  preserves tombstone fencing. Restored-fixture worker coverage passes 21/21;
  the library suite passes 147/147.
- Completed the browser-session OAuth ownership contract. New
  `session_activations` now create a `read write follow` token linked to the
  Mastodon `superapp` when available, safely falling back to a nullable
  application, and logout removes both the activation and its token. The
  guarded browser differential case verifies token ownership and cleanup.

## 2026-08-25

- Added Rails-compatible status-edit notification fan-out for local and remote
  notes. Edits now transactionally enqueue replacement-aware `update` jobs for
  local rebloggers and `quoted_update` jobs for accepted local quotes; the Core
  worker dispatches both activity types through the existing policy/resolution
  kernel.
- Added restored-fixture coverage for local and remote edit outbox production,
  durable notification delivery, duplicate handling, and cleanup. The schema
  integration passes 24/24 and the worker integration passes 20/20.
- Completed the basic moderation/reconciliation write path against the pinned
  Mastodon 4.6.5 fixture: report resolution, moderator status deletion,
  account suspension/unsuspension, domain block/unblock, account-stat repair,
  audit records, and durable side effects are transactionally covered.
- Added least-privilege writer validation for moderation tables and the
  operational outbox. Suspension now creates a local `moderation_warning`
  notification job, while direct statuses and non-public replies follow
  Mastodon's counter-cache rules during creation, deletion, and reconciliation.
- Added federated report forwarding as durable ActivityPub `Flag` jobs from the
  instance actor. Forwarding covers the target origin and distinct remote
  reply-server inboxes, matching Mastodon 4.6.5's inbox exclusions and nullable
  `forwarded` persistence semantics.
- Closed an account-status filtering gap: boosts whose source status is missing
  or soft-deleted are now omitted before pagination, matching Mastodon's
  `kept` scope. Restored-fixture coverage verifies the boost disappears when
  its source is deleted.
- Account-stat reconciliation now also repairs `status_stats.replies_count`
  for the target account's statuses from live reply rows, with restored-fixture
  corruption-and-repair coverage.
- Notification-request acceptance now queues a deduplicated durable Core job to
  clear filtered notifications from the accepted sender. Worker integration
  verifies the asynchronous effect, preserving Mastodon's request-acceptance
  timing without requiring Redis. Accepted filtered direct mentions now also
  rebuild sorted, unread `account_conversations` rows transactionally; repeated
  statuses in one conversation are merged idempotently. The least-privilege
  writer contract now includes `account_conversations_id_seq`, while Redis
  streaming merge publication remains deferred.
- Report creation now queues durable Mail-lane messages for eligible staff with
  the flat Mastodon `notification_emails.report` setting. The SMTP worker
  renders the report context and ID without advertising Rustodon's absent admin
  UI route; outbox and SMTP delivery are covered without exposing credentials.
  Report writes also enforce Mastodon's 400-per-UTC-day account limit with a
  PostgreSQL advisory lock, and report errors expose the rate-limit headers.
- Report validation now rejects attached statuses when the target blocks the
  reporter, requires rules for `violation`, and persists omitted `forward` as
  `false`. Writer preflight covers report collections/rules/outbox reads and
  report sequences, while rejecting dangerous explicit table grants.
- Restored-fixture validation now passes the 26-test schema suite, 20-test
  worker suite, 15 differential cases, 3 startup cases, preflight integration,
  and the aggregate `mise run check` gate. Full purge/severance, media privacy,
  timeline/streaming cleanup, and the admin report UI remain deliberately
  deferred.

## 2026-08-11

- Created the Rustodon repository structure and repository-local issue tracker.
- Defined Rustodon as a mostly-in-place replacement for small Mastodon
  installations, with persisted-data and protocol compatibility taking
  priority over Rails implementation parity.
- Selected Mastodon 4.6.5 as the first compatibility target. The initially
  inspected Mastodon `main` checkout identifies itself as 4.7.0-beta.1 and
  contains further schema and behavior changes that should be a separate
  compatibility milestone. Mastodon 4.6.5 already includes quote, collection,
  and keypair data, which v1 must read and preserve even though their mutation
  workflows are deferred.
- Scoped v1 around existing users, PostgreSQL, local Paperclip storage, core
  REST APIs, core ActivityPub federation, and durable PostgreSQL-backed work.
- Explicitly deferred object storage, Elasticsearch, open registration, SSO,
  polls, scheduled posts, quotes, relays, and full administration.
- Chose differential Rails-versus-Rust testing as the primary compatibility
  technique before porting individual Mastodon tests.
- Corrected the initial scope after independent review against the pinned
  Mastodon 4.6.5 tag and tightened physical-schema, signing-key, exclusive-list,
  and read-only acceptance criteria.
- Bootstrapped a single-crate Rust 2024 project with `web`, `worker`, and
  `admin` command surfaces, while deliberately leaving runtime behavior
  unimplemented.
- Pinned Rust 1.97.1, Clang 22.1.8, and cargo-deny 0.20.2 through Mise and added
  shared format, lint, test, dependency-policy, and aggregate check tasks.
- Added CI with immutable action revisions and prohibited unsafe Rust.
- Pinned the Mastodon v4.6.5 compatibility baseline to commit
  `1440d55b139e39ec722c2a3db7f60b66cd889048`, schema version
  `20260611150940`, and the official OCI index digest
  `sha256:77f11d1a6c674664217372d94ccdb9203524c60447827fe74ab6e11466825815`.
- Added a release-versioned, deterministic full database fixture with all 588
  migrations, a full PostgreSQL catalog fingerprint, Paperclip-shaped local
  media, explicit OAuth/relationship/filter/list/quote/collection/poll/keypair
  records, and readable activities for all 17 Mastodon 4.6.5 notification
  types.
- Kept Mastodon 4.6.5's signing-key contract: local account key material uses
  Mastodon's published test RSA key in `accounts`, while remote accounts and
  the remote `keypairs` record contain public material only.
- Identified four narrow fixture-generation normalizations: Mastodon's random
  `timestamp_id()` salt, Rails schema-load timestamps in `ar_internal_metadata`,
  PostgreSQL 14.23's random dump restrict token, and terminal dump formatting.
  The fourth removes only trailing empty `pg_dump` lines and enforces exactly
  one terminal LF while preserving all internal blank lines.
- Added environment-isolated, digest-pinned Podman tooling for source obtain,
  generation, static verification, clean restore/Rails verification, and
  byte-for-byte regeneration. Rails boot did not require a Redis container for
  the verified paths.
- Tightened the fixture after independent review: every Snowflake-backed row
  now encodes its deterministic `created_at` epoch and uses invocation-count
  sequence state; grouped notifications use Mastodon's exact target/hour keys;
  and the moderator role carries `manage_reports` and `manage_users` while each
  notification serializer receives its own recipient user.
- Replaced raw media copies with the pinned Mastodon image's v4.6.5
  Paperclip/libvips processors. The checked tree now contains a 400x400 avatar
  and distinct 600x400 original/588x392 small media styles with coherent
  metadata, blurhash, processing state, and verified output hashes.
- Forced all container runs to pinned `linux/amd64` child manifests, made source
  verification reject dirty trees, derived migration/media inputs from commit
  blobs, and replaced anonymous PostgreSQL storage with labeled volumes that
  are removed and checked after each task.
- Added a small read-only `rustodon::mastodon` compatibility library using
  dynamic SQLx PostgreSQL queries, Tokio, Chrono, JSON, prefix-preserving
  `inet`, and Saphyr YAML parsing. The private pool sets UTC and read-only
  session defaults; no write or Active Record callback surface is exposed.
- Represented IDs as signed `i64`, secrets as opaque redacted values, and
  visibility, notification, polymorphic, integer, and permission-bit values as
  lossless open/raw wrappers. Normal status reads exclude soft-deleted rows;
  notification reads hide filtered rows by default while retaining parent
  metadata when a target status has been deleted.
- Expanded the deterministic 4.6.5 seed with the `-99` instance actor, raw user
  JSON and array edge states, account JSONB, status edits, tag/conversation
  joins, missing v1 relationships and policies, Rails YAML tags, a tombstone,
  an unknown deleted status/filtered notification, and a valid deterministic
  Active Record encrypted keypair envelope. Rails still verifies all 17 known
  notification types separately.
- Added an ignored Podman-backed Rust schema integration task. Its random-port
  LOGIN role receives only `CONNECT`, `USAGE`, and `SELECT`; tests prove INSERT,
  UPDATE, DELETE, TRUNCATE, and schema creation remain forbidden even after the
  session read-only default is disabled.
- Aligned Saphyr 0.0.6 with SQLx's `hashlink` dependency line and pinned the
  compatible `indexmap` lock entry. Cargo-deny exceptions are limited to exact
  Redox, Syn, and Windows transitive versions selected by SQLx and the existing
  CLI stack.
- Closed schema-review gaps by preserving nullable account and notification
  columns, redacting OTP recovery codes, retaining arbitrary-precision JSON,
  and distinguishing unavailable local users from service and login accounts.
- Expanded account, OAuth, status, media, relation, and notification-activity
  mappings needed by the first REST and federation milestones. Direct status
  reads suppress soft-deleted rows while live parent records such as
  notifications and quotes remain lossless when a referenced status is gone.
- Added a test-only Rails-versus-Rust differential harness. It sends one typed
  request to distinct loopback targets, compares exact statuses, declared
  headers, canonical JSON, logical PostgreSQL snapshots, and media hashes, and
  reports focused JSON paths, table keys, and file paths.
- Kept the compatibility boundary on observable behavior rather than Rails
  internals: ActivityPub documents, durable job intent, and media artifacts have
  typed comparison slots, while callback counts, query ordering, Redis keys,
  and Sidekiq representation are deliberately excluded.
- Added exact, format-validating normalization rules for request IDs, generated
  timestamps, and prefixed random test tokens. Broad key deletion, wildcard
  paths, array reordering, and number coercion are not allowed.
- Added guarded differential orchestration using independently marked clones of
  the pinned database and media tree, a pinned empty Redis, and pinned Mastodon
  Puma. Rust receives SELECT-only clone credentials; loopback URLs, database
  comments, media markers, canonical paths, and symlink absence are validated
  before requests run.
- Added typed loading for the v1 Mastodon environment surface: canonical
  domains, PostgreSQL precedence, local Paperclip paths, trusted proxies, SMTP,
  cryptographic secrets, and optional Sidekiq Redis inspection. Secret wrappers
  zeroize on drop and redact `Debug`, `Display`, and validation failures.
- Added Rails 8.1 Active Record AES-256-GCM key decryption with current
  PBKDF2-SHA-256 and legacy SHA-1 read fallback, plus semantic RSA key matching
  and an in-memory RSA-SHA256 sign/verify check.
- Added `rustodon preflight` with stable fatal/warning codes. It compares all
  expected migration versions and the v1-critical physical PostgreSQL catalog,
  validates `timestamp_id()` and its seven sequences without executing them,
  checks canonical identifiers and operational local signing keys, and rejects
  active workflows outside v1.
- Kept preflight on the cutover contract rather than Rails implementation
  details. It rejects active object storage and SSO instead of parsing every
  provider option, ignores unrelated extension tables, and checks logical
  Sidekiq work rather than Redis's internal key layout beyond read-only queue
  discovery.
- Added read-only authentication for existing Mastodon OAuth bearer tokens with
  exact Doorkeeper revocation and expiration boundaries, endpoint-specific
  broad/granular scope alternatives, application-only principals, and Mastodon
  user/account functional-state ordering. The joined lookup never selects the
  bearer or refresh token, application secret, password, OTP material, or
  recovery codes.
- Deliberately omitted Mastodon's once-per-day access-token and user sign-in
  metadata writes. They remain deferred to the authenticated-write phase; the
  OAuth fixture and differential tests prove Rust authentication leaves every
  row unchanged. Differential setup refreshes that metadata only in its
  transient template before cloning so Rails does not introduce an expected
  tracking write during read-only response comparison.
- Added database-free REST serializers backed by batched read-only projections
  for Mastodon 4.6.5 accounts, credentials, relationships, statuses, media,
  polls, quotes, collections, filters, markers, notifications, and instance
  v1/v2 responses. IDs, dates, nullable fields, rendered HTML, authenticated
  state, and all 17 known notification types are differentially verified.
- Expanded compatibility fixtures for cached Paperclip media, profile mentions,
  historical null-local statuses, legacy null-type notifications, notification
  pagination/grouping stress, and authorization-sensitive quote states. Quote
  expansion is explicitly bounded and self-quote coverage proves cyclic data
  cannot recurse indefinitely.
- Promoted the first REST read surface to the production Axum web process:
  instance v1/v2 and rules, disabled translation languages, account show,
  lookup, verify-credentials, relationships, statuses, followers/following,
  status show, and context now read directly from PostgreSQL.
- Kept root `StatusPolicy` authorization, account-status selection, and context
  member filtering as distinct selectors. Current follows, active and silent
  mentions, author blocks and domain blocks, viewer blocks/mutes/domain blocks,
  suspended or silenced authors, and soft deletion are covered independently.
- Added stable Mastodon-compatible pagination for account statuses and follow
  collections, including `max_id`, `min_id`, `since_id`, endpoint limits, exact
  `Link` ordering, pin-time ordering, self-replies, and edited-out media.
- Expanded the deterministic fixture with pending/unconfirmed local accounts, a
  functional unrelated OAuth viewer, multiple follows and pins, blocked and
  domain-blocked thread members, and a silenced viewer's own reply. Dedicated
  SQL and HTTP authorization matrices prove private, direct, and limited status
  denial across show, account-status, and context endpoints.
- Hardened quote projection boundaries after independent review: unauthorized
  targets, including targets hidden by an author-side domain block, no longer
  enter nested status or target-link projections before serialization.
- Replaced Redis-derived home and list reads with direct PostgreSQL selectors
  and added public, hashtag, list, favourites, bookmarks, blocks, and mutes
  routes with endpoint-specific OAuth scopes and cursor contracts.
- Matched Mastodon timeline filtering for feed-access settings, follows and
  chosen languages, replies, boosts, exclusive lists, blocks, mutes, domain
  blocks, custom filters, hashtag normalization, and edited-out media.
- Expanded deterministic Rails feed materialization for followed tags and
  owner self-membership, then proved the timeline fixture byte-for-byte
  reproducible and all read responses differentially compatible with 4.6.5.
- Centralized shared REST protocol behavior around an explicit 21-route
  inventory, including CORS/preflight, trailing slashes, cache and `Vary`
  headers, Rails-compatible errors, request-size enforcement, and pagination
  contracts without advertising unsupported routes.
- Added database-referenced local Paperclip serving for accounts, media files
  and thumbnails, custom emoji, preview cards and provider icons, and site
  uploads, including existing processed audio/video paths. REST serializers and
  request authorization share cache-prefix, ID-partition, style, filename, and
  URL-escaping rules.
- Added Mastodon-compatible `GET`, `HEAD`, conditional, single-range, and
  streaming multipart-range responses with immutable cache, CSP, MIME,
  Last-Modified, and Rails-visible error behavior verified against pinned
  Mastodon 4.6.5.
- Hardened filesystem reads with a startup-retained `openat2` root descriptor,
  no symlink traversal, clean decoded components, regular-file checks, and
  best-effort no-atime reads. Differential snapshots now compare media mode,
  owner, group, and modification time in addition to bytes and hashes.
- Added bounded Rack-compatible query/form parsing and registered JSON body
  parsing with scalar, null, array, hash, collision, depth, count, byte-limit,
  numeric coercion, and body/query merge semantics verified directly against
  Mastodon 4.6.5.
- Closed the REST protocol milestone after independent review and sequential
  `check`, fixture restore/reproducibility, schema, preflight, and complete
  five-case differential gates all passed.
- Added an explicit, transactional `rustodon admin migrate-operational-schema`
  command for the separately owned `rustodon` namespace. Version 1 stores
  durable jobs, outbox events, idempotency keys, ordering markers, domain
  health, and worker/scheduler heartbeats without foreign keys or changes to
  Mastodon's `public` schema.
- Serialized operational DDL with both Rustodon and Active Record 8.1 migration
  locks, validated all 71 relations read by current Rustodon code plus the
  Snowflake sequences and `timestamp_id()` before and after DDL, and rejected
  event triggers, all-table publications, behavior hooks, unsafe collations,
  unpopulated materialized views, and unsupported Mastodon schema versions.
- Pinned an OID-independent operational catalog fingerprint that records the
  original schema owner, complete ACLs, schema-qualified collations, comments,
  dependencies, extension membership, triggers, rules, policies, and other
  PostgreSQL 14 namespace object classes. Fresh, repeat, concurrent absent and
  empty-schema creation, owner reassignment, grants, and attached-object drift
  are covered by the isolated integration gate.
- Closed the operational-schema milestone after independent review and
  sequential `check`, operational/preflight integration, fixture
  restore/reproducibility, schema integration, and all six differential cases
  passed. Before/after catalog, schema, data, owner, and ACL snapshots plus
  pinned Rails verification prove Mastodon rollback remains possible.
- Added PostgreSQL durable jobs and transactional outbox dispatch with delayed
  execution, fenced renewable leases, final-attempt crash recovery, logical-key
  deduplication/cancellation, bounded deterministic jitter, dead letters, and
  worker/scheduler heartbeats. Dispatch and cancellation serialize on keyed
  outbox rows so a committed cancellation cannot leave runnable work behind.
- Added a handler registry with explicit lane capability and independent remote
  HTTP/media semaphores. Leases renew while waiting for permits; stale handlers
  cannot acknowledge expired or replaced leases. Infrastructure currently
  registers only maintenance cleanup, making `maintenance` the truthful default
  and rejecting configured lanes without handlers.
- Required a distinct `NOINHERIT` runtime database login with exact operational
  DML/sequence grants and read-only Mastodon access. Startup rejects schema
  owners, memberships, database/schema creation, direct or `PUBLIC` Mastodon
  writes, operational ACL drift, and unsupported schemas.
- Added readiness and bounded dead-letter administration, poll-cadence outbox
  draining independent of heartbeat cadence, and scheduler/handler shutdown
  separation. Shutdown joins the sole heartbeat writer, withdraws readiness,
  drains handlers within one absolute deadline, and never acknowledges aborted
  work.
- Closed the durable-worker milestone after independent review and sequential
  `check`, worker/operational/preflight integration, fixture restore and
  reproducibility, least-privilege schema integration, and all six differential
  cases passed. Eight PostgreSQL integration cases cover queue concurrency,
  final-attempt crashes, duplicate effects, permit-wait renewal, dispatch versus
  cancellation, retries/dead letters, runtime privileges, readiness, and
  shutdown.
- Added shared production startup validation before web bind or worker claims.
  The bounded, read-only checks cover configuration, media, the pinned Mastodon
  schema, signing keys, canonical domains, active workflows, the operational
  schema, the direct runtime role, and its required Mastodon and operational
  privileges. Operational migrations remain explicit and perform no runtime DDL.
- Added strict listener parsing, graceful serving, dependency-free `/health`,
  and bounded `/ready` checks for database availability plus `SELECT` on every
  v1-critical Mastodon relation. Worker startup publishes initial worker and
  scheduler heartbeats before claim loops begin.
- Added explicit trusted-proxy handling: forwarding metadata is ignored unless
  `TRUSTED_PROXY_IP` trusts the peer, malformed trusted forwarding fails closed,
  and effective authorities are constrained to configured canonical and media
  hosts. Absolute Paperclip media uses the sanitized effective authority.
- Closed production startup safety after independent review and sequential
  `check`, startup, worker, operational/preflight integration, fixture restore
  and reproducibility, least-privilege schema integration, and all six
  differential cases passed. Real-process tests prove fatal startup has no web
  bind or worker side effects and readiness degrades after database or required
  relation privilege loss without affecting liveness.
- Began the cross-surface policy foundation with typed, pure status audience and
  context decisions shared by root reads, context filtering, and quote targets.
  Unknown visibility, deletion, and suspension fail closed before owner
  exemptions; private/direct audiences preserve Mastodon 4.6.5 semantics.
- Made raw status graph loading private so public root reads must authorize
  first. Added an isolated database mutation regression proving an undeleted
  unknown visibility cannot appear as a root, context member, quote target, or
  shallow target ID. Seven schema integration cases and the focused Mastodon
  differential authorization matrix passed, and independent review found no
  blocker in this slice.
- Added exact Mastodon 4.6.5 local-role semantics for all 23 permission bits,
  including EVERYONE inheritance, direct administrator expansion, any-of
  permission checks, strict position hierarchy, and highlighted moderation block
  bypass. Raw and effective masks are distinct types so action policy cannot
  accidentally skip inheritance.
- Keyed credential and restricted-feed permission loading to the exact OAuth
  resource-owner user/account pair. Crossed identities fail closed, and exact
  owner settings and role now drive both flattened account fields and top-level
  credentials. Startup and preflight reject a missing mandatory EVERYONE role.
  Eight schema integration cases, preflight clone rejection, Clippy, and an
  independent role-policy review passed.
- Added typed account lifecycle decisions for limited, moved, memorial,
  temporary suspension, and permanent unavailability. Browser authentication
  is intentionally distinct from functional API access; OAuth reads deletion
  requests and keeps the instance-actor suspension exemption.
- Added reusable global-domain decisions with Mastodon-compatible transitional
  IDNA normalization, exact label boundaries, longest-parent precedence,
  silence/suspend/noop and media/report controls, and fail-closed unknown or
  NULL severity. Independent comparison against the pinned Mastodon lifecycle
  and domain models found no blocker; federation call sites remain owned by the
  later signature, fetch, inbox, and delivery slices.
- Began the public federation discovery slice with WebFinger, host-meta, NodeInfo
  2.0, local actors, public Notes, outbox, followers, and following collections.
  Local ActivityPub URLs derive from the account ID scheme, local endpoint fields
  are derived when legacy rows are blank, accepted quote targets flow through the
  existing HTML formatter, and browser requests preserve Mastodon's absolute
  HTML redirects instead of returning JSON-LD.
 - Added a guarded `federation_discovery` Rails-versus-Rust differential case for
   malformed and unknown WebFinger resources, discovery documents, actor and
   Note fields, collection totals, pagination identifiers, and HTML redirects.
   The case passes against the pinned Mastodon 4.6.5 fixture. The fixture's
   `/actor` request currently returns HTTP 500 despite the pinned upstream request
   spec requiring 200, so that inconsistent instance-actor request remains a
   separate follow-up rather than weakening the supported discovery gate.
- Hardened the discovery slice after source review: unavailable actors mask
  profile fields, WebFinger authorities preserve explicit ports, local quote
  URIs respect numeric account IDs, suspended collection members remain
  representable, collection page presence matches Rails, ActivityPub Accept
  negotiation honors quality values, and outbox data errors fail closed.
- Restored Rails prefix coercion for REST route IDs while keeping ActivityPub
  account IDs constrained, and added the encoded-ID regression to the guarded
  federation case. Formatting, Clippy, 42 unit tests, all 7 differential cases,
  and startup integration pass.
 - Moved `GET /api/v1/collections/:id` from a differential-only fixture route
   into production routing, reusing the visibility-aware collection projection.
   Added exact status-source serialization and status-history snapshots, including
   historical media ordering/descriptions, polls, legacy quote states, and strict
   token handling. The core REST differential now exercises all three production
   endpoints and remains compatible with Mastodon 4.6.5.
 - Added reverse status actor reads for favourites and boosts. Their selectors
   preserve Rails association/status cursor IDs, public/unlisted root policy,
   suspended-account exclusion, viewer block/mute filtering, application-only
   token behavior, and ordered pagination links.
 - Added production account search with required user authentication, exact
   stored local/remote handle matches, PostgreSQL full-text ranking, following
   filtering, and Rails-compatible limit/offset behavior. `resolve=true` is
   explicit for complete remote handles, keeping network resolution isolated
   behind the later safe remote-fetch milestone.
 - Moved `GET /api/v1/markers` into production routing. It preserves
   user-scoped marker ownership, scalar/array/unknown timeline semantics,
   private cache/Vary behavior, and the existing Rails differential/auth matrix;
   marker writes remain deferred.
 - Moved `GET /api/v2/filters` into production routing, reusing the existing
   account-scoped filter, keyword, and status projections. The endpoint now
   participates in the production protocol path; filter writes remain deferred.
 - Added production `GET /api/v1/lists` using the existing account-owned list
   query. The four-field response, replies-policy mapping, trailing slash,
   private headers, and auth/owner-isolation cases are differentially covered;
   list writes remain deferred.
 - Added production `GET /api/v1/featured_tags` with account-owned tag joins,
   tag-name fallback, account-tag URLs, string counts/date serialization, and
   broad/granular/owner-isolation differential coverage.
 - Added public production `GET /api/v1/accounts/:id/featured_tags` with
  unavailable/suspended-account handling, public access independent of token
   scopes, account-tag URLs, and anonymous/trailing/missing-target coverage.
 - Added public production `GET /api/v1/custom_emojis` using the Rails `listed`
   scope: local, enabled, picker-visible emojis only, with category/featured
   metadata and Paperclip URLs. Anonymous and authenticated trailing-slash
   responses are covered by the Rails differential suite.
 - Added production `GET /api/v1/featured_tags/suggestions` with required
   account-read authentication, Rails recent-status ranking, featured-tag
   exclusion, relationship booleans, trailing-slash support, and differential
   coverage. The old fixture-only authentication route was removed.

## 2026-08-21

- Moved `GET /api/v1/notifications` and `GET /api/v2/notifications` from
  differential-only fixture handlers into production routing. Shared read
  projections now cover Rails type/exclusion filters, filtered rows, v1/v2
  cursor pagination, grouped-type selection, partial avatars,
  repeated array parameters in `Link` headers, and trailing slashes.
- Added differential coverage for notification pagination, grouping, exclusions,
  fallback serialization, and both production route versions. The focused core
  REST case passes against Mastodon 4.6.5; notification writes, clear, and
  dismiss remain deferred.
- Added production v1/v2 notification unread-count and show reads. Counts honor
  notification markers, type exclusions, limits, and v2 grouped types; show
  reads enforce account ownership and preserve v1/v2 serializer shapes. Clear
  and dismiss remain blocked on the writable repository foundation.
- Closed the remaining notification read parameter-parity gaps for blank scalar
  grouped types and scalar supported types, then passed the full local check,
  all seven guarded read differential cases, and startup safety integration.
- Began the writable transaction foundation with a separate typed
  `WriteRepository`. Marker updates require a functional `write:statuses`
  bearer, use Rails-compatible `INSERT ... RETURNING` and optimistic locking,
  and are covered by owner-role schema integration. Atomic idempotency claims,
  JSON result recording, optional outbox composition, a least-privilege writer
  role, and a guarded Rails-versus-Rust marker differential case now pass; web
  startup now accepts an explicit optional `WRITE_DATABASE_URL` while retaining
  the read-only default, and user-facing writes remain deferred.
- Added v1/v2 notification clear and dismiss POST routes. They require
  `write:notifications`, delete only the authenticated account's rows, handle
  grouped keys, and reconcile filtered notification requests transactionally.
   The guarded differential suite now covers these writes alongside the
   marker foundation.
- Added an internal idempotent notification-creation kernel for mention, status,
  follow-request, and poll activities. It validates recipient ownership, drops
  unavailable/self/blocked/muted/domain-blocked/conversation-muted activities,
  applies the supported notification-policy actions, rejects silent or deleted
  source activities, and updates filtered mention requests with capped counts.
  Owner-role schema integration now covers retry idempotency and recipient
  validation; policy unit tests cover accept/filter/drop precedence.
- Tightened the differential writer role with read-only access to the source and
   policy tables needed by notification creation while retaining write access
   only for markers, notifications, and notification requests. The full local
   check, startup integration, schema and operational integrations, and all nine
   guarded differential cases pass after these changes.
- Expanded the notification creation kernel to all 17 stored Mastodon 4.6.5
  activity/type associations. Groupable favourite, reblog, follow, and admin
  sign-up notifications now use Rails-compatible target/hour keys; update,
  quoted-update, and collection-update retries replace prior rows. Added
  recipient validation, staff mention bypass behavior, filtered quote request
  updates, ungrouped v2 dismissal, exact clear/delete-all behavior, and
  full owner-role integration coverage.
- Added `POST /api/v1/markers` with Rails nested parameters and trailing-slash
  routing. Multi-timeline updates run in one writer transaction, return exact
  marker serializers, and participate in the guarded differential suite.
 - Added account-owned conversation index, read, unread, and delete endpoints.
   Conversation serialization now loads sorted participants and visibility-aware
   last statuses, uses account-conversation IDs, preserves Rails deleted-last-
   status validation, and participates in the notification/write differential
   case with database restoration.

## 2026-08-22

- Persisted notification group buckets in Rustodon-owned expiring ordering
  markers so dismissing the final row preserves Rails' 12-hour grouping window.
  The owner-role schema regression and least-privilege notification differential
  case pass without changing Mastodon's public schema.
- Added production `GET /api/v1/followed_tags` with account-owned cursor
  pagination, Rails-compatible legacy `follow` scope support, tag relationships,
  empty seven-day history, trailing-slash routing, and differential coverage.
- Added production `GET /api/v1/follow_requests` with suspended-requester
  filtering, full account serialization, legacy follow-scope authentication,
  cursor pagination, trailing-slash routing, and differential coverage.
- Added production `GET /api/v1/preferences` with exact user/account settings
  ownership, Rails preference defaults and locale fallback, trailing-slash
  routing, and differential coverage.
- Added the reusable legacy Cavage HTTP signature primitive: RSA-SHA256 GET/
  POST signing and verification, exact body digests, required signed headers,
  one-hour clock skew, explicit key-ID binding, `expires` enforcement, and
  Mastodon's queryless request-target fallback. Fixed Mastodon GET/POST vectors,
  malformed cases, digest tampering, and private-key Debug redaction are
  covered; RFC 9421 and `hs2019` remain explicitly unsupported until transport
  integration is designed.
- Re-ran the full local check, 7 HTTP-signature tests, all 9 guarded
  differential cases, 12 schema tests, operational-schema integration, and
  startup safety integration successfully after the signature slice.
 - Added persisted ActivityPub public-key resolution for canonical actor IDs and
   legacy `acct:` aliases, retaining ownership, revocation, and expiry metadata.
   Existing actor GETs now optionally verify legacy signatures before rendering;
   the signed actor request passes the pinned Mastodon 4.6.5 differential case.
   Inbox enforcement, remote fetching, outbound signing, and broader signed-GET
   policy remain intentionally separate transport milestones.

- Added the first production status-interaction writes: bookmark and favourite
  create/remove operations use the optional least-privilege writer, preserve
  original-status targeting, maintain favourite counters, and create supported
  favourite notifications. Rails' POST `unbookmark` and `unfavourite` routes
  are mirrored explicitly; the writer fixture grants only the required public
  tables and sequences.
- Added owner-role schema coverage and folded four interaction requests into the
  guarded write differential case, including database snapshots and restoration.
  The full local check, 14-test schema integration, operational schema and
  startup integrations, and all 9 guarded differential cases pass. Boost writes,
  concurrent interaction proof, and complete interaction/outbox behavior remain
  open under the status-social-interactions issue.
- Extended the interaction writer with Rails-compatible POST reblog/unreblog
  routes. Boost creation now uses a hashed transactional advisory lock, the
  Mastodon timestamp ID function, account visibility defaults, original-status
  reblog counters, account status counts, and soft removal on unboost. Schema
  coverage proves duplicate creation and removal are idempotent; the guarded
  HTTP case covers generated response shape and restoration. Rails' second
  asynchronous unreblog response remains excluded from exact body comparison
  until its worker-dependent serializer behavior has a stable contract.
 - Started account relationship writes with local follow/unfollow routes. The
   writer preserves reblog/notification/language options, chooses direct follows
   versus pending requests, locks relationship pairs, updates both account
   counters, and emits the supported local follow notification. Schema and
   guarded differential coverage pass for duplicate retries and restoration;
   block, mute, follow-request transitions, and remote delivery remain open.
 - Added local block/unblock and mute/unmute writes with Rails scope contracts,
   transactional advisory locking, idempotent retries, follow cleanup on block,
   notification visibility flags, and nullable mute expiration. Expanded the
   deterministic fixture's follow token to cover the new write scopes, regenerated
   the database dump, and passed 16/16 owner-role schema tests plus all 9 guarded
   differential cases. Active mute expiry/worker cleanup, follow-request
   transitions, remote delivery, and complete outbox intent remain open.
 - Added local follow-request authorize/reject and remove-from-followers routes.
   Authorization preserves request options and URI while moving the request to a
   follow, updates both account counters, removes the dependent request
   notification, and emits the supported local follow notification. The 17-test
   owner-role schema suite and all 9 guarded differential cases pass with source
   and target relationship rollback; mute expiry, block cleanup, federation,
   and outbox behavior remain open.
 - Added the first status-write lifecycle slice: text-only `POST /api/v1/statuses`
   supports all five visibility values, content warnings, sensitivity, language,
   application attribution, quote-policy defaults, conversations, status stats,
   account counters, and operational idempotency replay. The 18-test owner-role
   schema suite and all 9 guarded differential cases pass; media, replies,
   mentions/tags, edits, deletes, and distribution remain open.
 - Extended text status creation to authorized replies, inheriting the target
   conversation and language while updating parent reply counters. Differential
   coverage now creates all five visibility variants plus a reply and restores
   generated statuses, conversations, status statistics, and account counters.
- Hardened the status differential cleanup to delete generated per-target rows
  instead of mutating the fixture's referenced status. The guarded notification
  case now remains isolated after status deletion, with the full 9-case suite
  passing again.
- Extended status deletion to discard reblogs, remove status pins, and maintain
  the owner's status counter transactionally. Owner-role schema coverage now
  proves reblog and pin cleanup, while the least-privilege writer grants the
  required `status_pins` table and operational outbox sequence.
- Scoped status idempotency keys by account, bound fingerprints to reply
  targets, and evicted expired keys during claims. Unit and owner-role schema
  tests cover account/reply identity, canonical whitespace, and fresh creation
  after expiry.
- Removed stale follow, follow-request, favourite, and reblog notifications
  during relationship teardown. Differential interaction coverage now snapshots
  affected notification rows and verifies rollback to the Rails baseline.
- Block creation now clears the matching notification-permission exception
  transactionally, with owner-role schema coverage and least-privilege grants
  proving the cleanup.
- Notification policy now treats expired notification-hiding mutes as inactive;
  an isolated owner-role regression covers the expired-mute path.
- Added the text/settings/fields slice of `PATCH /api/v1/accounts/update_credentials`
  with `write:accounts` authorization, transactional account and user-setting
  updates, profile field verification preservation, attribution-domain
  normalization, bot/visibility flags, and Rails-compatible profile hashtag
  refresh. Local profile field links now carry the `rel="me"` contract.
- Added nested form parsing, owner-role schema coverage, least-privilege grants,
  and Rails differential coverage for profile updates, including wrong-scope
  rejection and rollback. The guarded schema suite is now 20/20, the full
  differential suite remains 9/9, and the repository check passes; avatar/header
  media processing and removal remain open.
- Added local avatar/header multipart updates and the Rails-compatible profile
  deletion routes. Paperclip writes are path-safe and non-destructive, filenames
  are obfuscated and MIME-normalized, avatar/header styles enforce decoded image
  limits, GIF originals retain their frames, and static GIF derivatives are
  generated. Owner-role metadata coverage, 11 Paperclip tests, the 52-test local
  suite, guarded schema 20/20, guarded differential 9/9, and `mise run check`
  pass. HTTP media differential rollback and ActivityPub/profile side-effect
  workers remain open.
- Added image-only v1/v2 media CRUD with `write:media` ownership checks,
  descriptions, focus metadata, 8,294,400-pixel original limits, 230,400-pixel
  small derivatives, Paperclip paths, generated blurhash values, and safe
  database/file cleanup. Added media writer grants, route inventory coverage,
  Paperclip tests, and an owner-role create/update/delete test. The guarded
  differential case now covers v1 create/update/delete, blank-focus no-op
  updates, v2 image create, readable original/small artifacts, and rollback;
  all 10 cases pass. Blank/null focus now follows Rails' no-op setter behavior,
  and full-suite fixture orchestration makes media roots writable only when the
  media write case is included. Animated-GIF/video transcoding, HEIC/AVIF
 conversion, exact libvips byte/blurhash parity, and crash-time database/file
 compensation remain open.
- Extended status creation with Rails-compatible ordered owned-media attachment,
  media-only posts, omitted-language fallback to `en`, hashtag persistence, and
  featured-tag counter updates. Existing local/remote account mentions now
  persist mention rows and invoke the notification policy kernel. Owner-role
  schema coverage and the full 10-case differential suite pass with
  status/media/tag/mention rollback; remote account resolution, edits,
  delete-media handling, and post-commit distribution remain open.
- Added the next status lifecycle slice with owner-authenticated `PATCH
  /api/v1/statuses/:id`. Text, content warnings, sensitivity, language, and
  ordered media changes now run transactionally with Rails-compatible initial
  and current `status_edits` snapshots, hashtag/featured-tag refresh, mention
  replacement, and policy-aware mention notifications. Differential coverage
  compares edited HTTP/database state, history row counts, and media removal;
  `mise run check`, owner-role schema 21/21, and all 10 guarded differential
  cases pass. Remote account resolution, delete-media semantics, explicit
  idempotency-key edit replay, and post-commit distribution/removal remain open.
- Added Rails-compatible `PUT /api/v1/statuses/:id` as an alias of the status
  update action. The guarded differential case now proves identical PUT replay
  is a no-op with no extra edit snapshot; owner-role schema coverage proves the
  same repository behavior. The image media CRUD and account profile update
  issues now satisfy their written acceptance criteria and were archived, while
  codec, federation, worker, and crash-hardening follow-ups remain open.
- Audited timed mute cleanup against the durable worker boundary. Worker
  integration confirms the runtime role is intentionally read-only on Mastodon
  tables, so expiry cleanup cannot be added by granting direct deletes; it
  requires a separate configured writer pool and remains open under the
  relationship-write issue.
 - Added the writer-backed timed mute expiry slice. Mute writes now record an
   atomic `rustodon.mastodon.delete_mute` outbox event and remove pending events
   on mute removal or renewal; workers execute expiry through an optional
   separately configured Mastodon writer pool while retaining a read-only
   default runtime role. Owner-role schema, worker integration, and focused
   differential verification pass. Remote relationship transitions, block
   side effects, complete outbox intent, and concurrency proof remain open.
 - Added transactional status `delete_media` handling. Owner-authenticated
   deletion now detaches or removes unreported media, retains media for
   unresolved reported statuses, removes pins, reconciles original/reblog
   counters and owners, and casts JSON numeric `0` as false. Owner-role schema
   coverage, the focused Rails-versus-Rust HTTP case, the full repository check,
   and all 10 guarded differential cases pass for both deletion modes;
   asynchronous distribution/removal remains in the parent lifecycle issue.
 - Completed local block teardown. Block writes now share the recipient
   notification advisory lock, preserve outgoing follow requests, reject
   incoming requests, and atomically clear the block owner's notifications,
   notification requests, and conversations containing the blocked account.
   Self-block is a Rails-compatible no-op; owner-role schema, relationship
   differential coverage, the full repository check, and all 10 guarded cases
   pass. Remote delivery and concurrent relationship proof remain open.
 - Added concurrent follow, block, and mute requests to the relationship
  differential case. The proof compares stable final relationship options,
  counters, synchronous notification cleanup, and Rails' unique-validation
  loser responses, restoring both fixture databases after each case. Remote
  relationship transitions and asynchronous outbox delivery remain open.
  - Added concurrent bookmark, favourite, and reblog create/remove coverage to
    the status interaction differential case. Rails' exact duplicate-record
    loser response is required, generated reblog IDs are normalized only after
    shape validation, and final assertions reject duplicate rows while comparing
    status/account counters. Both fixture databases restore interaction,
    conversation, notification, and notification-request state; queued Rails
    favourite/reblog removal effects are drained explicitly because the guarded
   fixture has no Sidekiq consumer. Owner-role schema 21/21, repository check,
   and all guarded differential cases passed at that point.

## 2026-08-23

- Added production `GET /api/v1/lists/:id`, `GET
  /api/v1/lists/:id/accounts`, and `GET /api/v1/accounts/:id/lists` reads.
  List ownership, `read:lists` scope alternatives, suspended-member filtering,
  cursor pagination, Rails' `limit=0` ordering, account-list membership, and
  trailing-slash routes now match the pinned fixture. Added guarded differential
  coverage for ownership, scope failures, missing records, cursors, and
  unlimited results.
- Added direct media-show differential coverage so the production
  `GET /api/v1/media/:id` response is checked after create/update and before
  deletion. The focused core and media differential cases pass after the slice.
- Added production account-owned and featured-in collection reads for the
  preserved collection data, including Rails authentication differences,
  offset pagination, discoverability/suspension rules, collection envelopes,
  and trailing-slash routes. Core differential coverage now exercises anonymous,
  authenticated, missing-record, and wrong-scope cases.
- Added production notification-request list/show reads with Rails-compatible
  account ownership, functional-user authentication, request/status graphs,
  max/min/since cursor behavior, and bounded pagination links. Added the
  persisted `updated_at` projection needed by the REST serializer; core
  differential coverage now includes list, show, cursor, scope, and missing
  request cases.
- Added production notification-request accept/dismiss member and bulk POST
  routes. Immediate Rails effects now match: owner-scoped member 404s, ignored
  missing bulk IDs, permission insertion on acceptance, request deletion, and
  `{}` responses. Recipient advisory locks serialize decisions with filtered
  notification creation. Differential coverage compares auth failures,
  scalar/array IDs, permission/request state, and unchanged notifications under
  the fixture's no-Sidekiq boundary; asynchronous unfilter/cleanup workers
  remain a separate lifecycle follow-up.
- Added production `GET /api/v1/statuses/:id/quotes`. Accepted quote sources
  now use root-status authorization, visibility-aware status projection graphs,
  Rails' status/quote ordering and quote-ID cursors, required
  `read:statuses` authentication, private protocol headers, and trailing-slash
  routing. The core Rails differential covers success, cursors, missing auth,
  and wrong scopes.
- Added authenticated `GET /api/v1/notifications/requests/merged` in both
  slash forms. It reports the synchronous settled state while the
  Redis-backed notification unfilter worker remains deferred; differential
  coverage now includes success, trailing slash, missing auth, and wrong scope.
- Added authenticated v1/v2 notification-policy reads. The serializers preserve
  Rails defaults, v1 boolean compatibility, v2 accept/filter/drop strings,
  unknown persisted values, pending-request summaries, suspended-sender
  filtering, and trailing-slash behavior. Differential and owner-role schema
  coverage now prove the policy projection.
- Added transactional v1 boolean and v2 enum notification-policy updates with
  account advisory locking and default-preserving upserts. The notification
  write differential now compares auth failures, response bodies, policy state,
  and rollback restoration for both API versions.
- Added status conversation mute/unmute writes with `write:mutes` authorization,
  idempotent persistence, status response serialization, trailing-slash routes,
  and guarded interaction rollback coverage.
- Added owner-only status pin/unpin writes with `write:accounts` authorization,
  Rails validation boundaries, idempotent pin persistence, trailing-slash routes,
  and guarded rollback coverage.
- Added public `POST /api/v1/apps` registration with Doorkeeper-compatible
  credential persistence, secure generated uid/secret values, default scopes,
  redirect URI normalization, VAPID metadata, trailing-slash routing, and
  differential rollback coverage that preserves existing OAuth tokens.
- Added authenticated `GET /api/v1/apps/verify_credentials` in both slash
  forms, with public-only application serialization and differential coverage
  for application-backed, application-only, scope-independent, revoked,
  expired, unknown, and missing bearer tokens. Application credentials now use
  Doorkeeper's unpadded URL-safe Base64 shape over 32 random bytes.
- Tightened app registration to Mastodon's 60/2,000/2,000 field limits,
  configured scope set, OOB redirect exception, URL scheme/host checks, and
  fragment/relative/forbidden-scheme rejection. Distributed registration
  throttling, optional VAPID nulls, and parameter-based bearer transports remain
  deferred with the rest of the OAuth server.
- Matched Mastodon's app-scope edge behavior: array-form scopes fall back to
  `read`, and scalar scopes are deduplicated in original order before storage.
- Added the browser-independent `POST /oauth/token` client-credentials grant.
  Client-secret POST and Basic authentication, default and application-bounded
  scopes, active-token reuse, new application-only token persistence, exact
  Doorkeeper response headers, and cleanup through application deletion now pass
  guarded Rails differential coverage.
- Added `POST /oauth/revoke` for client-authenticated access-token revocation,
  ownership enforcement, token-type hints, unknown-token idempotence, and
  persisted `revoked_at` state.
- Added `GET`/`POST /oauth/userinfo` with profile-scope authorization and
  Mastodon-compatible OIDC claims, cache policy, and `Vary` behavior.
- Added `GET /.well-known/oauth-authorization-server` with the pinned
  authorization, token, userinfo, revocation, registration, scope, grant, and
  PKCE metadata. The guarded OAuth differential case compares the exact JSON
  and cache headers.
- Added Doorkeeper-compatible `access_token` and `bearer_token` parameter
  authentication to the API middleware for both query and form parameters.
  Explicit `Authorization` headers remain authoritative, and the focused
  bearer-authentication differential case passes against Mastodon 4.6.5.
- Added browser authentication foundations: bcrypt password verification,
  SHA-1 TOTP with replay protection, backup-code matching and consumption,
  durable login activity limits, transactional `session_activations`, secure
  session/CSRF cookies, and `/auth/sign_in`, `/auth/session`, and sign-out
  routes. Guarded browser-auth differential vectors pass; WebAuthn, HTML form
  rendering, recovery mail, and distributed limits remain open.
- Added the OAuth authorization-code path: browser-session consent at
  `/oauth/authorize`, one-time persisted grants, S256 PKCE verification,
  redirect/state handling, denial responses, and authorization-code token
  exchange. Guarded differential coverage proves token issuance, grant
  revocation, replay rejection, and response compatibility.
- Expanded the authorization-code differential case through `/oauth/userinfo`
  using the generated profile-scoped token. Added fixture cleanup privileges
  for authentication and OAuth rows so the full 12-case differential suite
  runs concurrently without leaking state; all 12 cases pass.
- Added transactional password recovery foundations: bcrypt administrator reset
  through `rustodon admin reset-password`, single-use SHA-256 reset tokens with
  six-hour expiry, CSRF-protected reset request/edit/update pages, session and
  OAuth-token invalidation, and guarded request/update/replay/expiry coverage.
  The full guarded differential suite now passes 13/13 cases. SMTP confirmation
  delivery and CLI user creation remain open.

## 2026-08-24

- Added durable local status-notification production. Status creation now records
  a transactional `rustodon.mastodon.notify_status` Core-lane outbox event;
  configured workers select active local followers with `notify=true`, restrict
  limited/direct delivery to mentioned followers, and reuse the idempotent
  notification-policy kernel. Deletion cancels pending events and retries do not
  duplicate rows. Owner-role schema coverage now proves active-follower delivery
  and the guarded notification write case passes; feed fan-out, edit/quote
  updates, streaming, and remote delivery remain separate milestones.
- Made optional VAPID configuration explicit in the instance runtime model.
  OAuth application registration and credential verification now emit `null`
  when no public VAPID key is configured instead of an empty string, while the
  configured-key differential shape remains unchanged.
- Added fail-closed startup validation for an optional `WRITE_DATABASE_URL`.
  The bounded read-only catalog check rejects unsafe role attributes,
  ownership, membership, database/schema creation, writable defaults, and
  missing status/notification/outbox capabilities. Operational-schema ACL
  comparison ignores only the explicitly configured writer role. Guarded
  startup proves valid web/worker use and refusal before bind or work claim for
  unsafe writer configurations; the writer-privilege issue is archived.
- Re-ran `mise run check`, `mise run startup-integration`, `mise run
  worker-integration`, and the full `mise run differential` suite. The local
  gate passes 66 tests and the guarded differential suite passes 13/13.
- Completed the account recovery and mail slice. Durable reset and confirmation
  jobs now use AES-256-GCM envelopes in the PostgreSQL outbox, with SMTP lane
  delivery through lettre, explicit retryable transport failures, TLS/custom-CA
  handling, reply-to/return-path support, and the configured canonical origin.
- Added Devise-compatible PBKDF2-HMAC-SHA1 key derivation with
  column-specific salts and HMAC-SHA256 token digests for new SMTP-backed reset
  and confirmation records. Legacy SHA-256 and raw confirmation lookups remain
  available for existing Rails/Rust-owned rows. Reset tokens remain six-hour,
  single-use values; confirmation tokens expire after two days.
- Added `/auth/confirmation` and wired browser reset requests/updates to the
  durable outbox. Password reset now validates the 8..72-byte password policy,
  looks up the token before bcrypt work, clears sessions and stale sign-in data,
  and revokes both OAuth access tokens and pending authorization grants.
- Added `rustodon admin create-user` with normalized email/username validation,
  stdin password fallback, bcrypt credentials, generated RSA signing keys,
  initial `account_stats`, and transactional confirmation-mail enqueueing when
  SMTP is enabled. SMTP-disabled administration creates a confirmed user.
- Recovery refuses external-auth users, deletes push subscriptions while
  revoking OAuth access, and applies local-process IP/email reset throttles at
  Mastodon's 25-per-five-minute and 5-per-thirty-minute limits. Cross-process
  distributed throttling and live streaming kill events remain deployment-level
  follow-ups.
- Added mail encryption/retry, SMTP lane, CLI, digest, password-boundary, and
  guarded OAuth-grant recovery coverage. `cargo test --locked --all-targets
  --all-features`, Clippy with warnings denied, formatting, and cargo-deny all
  pass; fixture-backed integration remains gated by the unavailable restored
  PostgreSQL/Mastodon environment.
- Restored the pinned PostgreSQL/Mastodon fixture and removed the recovery
  integration gate. The guarded Rails-versus-Rust suite now passes 14/14,
  including an `admin_create_user` case that invokes both administrative CLI
  commands through stdin, verifies bcrypt credentials, `account_stats`, the
  encrypted confirmation outbox event, and confirmation success, replay
  rejection, and expiry. `mise run fixture-restore-verify` and
  `mise run fixture-verify` also pass.
- Live SMTP delivery remains unverified because no SMTP service is available.
  A local SMTP protocol integration now exercises the actual worker runtime and
  verifies the decrypted reset link in the received message. The refused-
  connection unit test still proves transport failures are retryable;
  cross-process reset throttling and streaming kill events remain
  deployment-level follow-ups.
- Added process-local browser login throttles matching Mastodon 4.6.5: 25
  attempts per client IP in five minutes and 25 attempts per normalized email
  in one hour. Focused limiter boundaries and the full 14-case differential
  suite pass; shared-store throttling remains a deployment-level follow-up.
- Added the Mastodon 5-per-10-minute client-IP throttle for OAuth application
  registration. Attempt windows now use fixed epoch buckets with bounded state,
  and throttled browser/API responses expose reset and retry headers.
- Added the first safe remote-fetching foundation: strict HTTP(S) URL checks,
  Mastodon-compatible private/documentation/reserved address rejection,
  mixed-answer DNS validation, fixed address pinning, redirect revalidation,
  disabled proxies, content-type checks, timeout bounds, and streaming body
  limits. ActivityPub profile parameters, exact `200 OK` responses, canonical
  JSON identity matching, compressed-response rejection, and hard configuration
  ceilings are tested. Signed GET transport and adversarial network fixtures
  remain open for the next federation milestone.
 - Added a typed WebFinger/ActivityPub actor resolver layer with canonical
  subject/origin checks, ActivityStreams context validation, supported actor
  types, safe endpoint shapes, profile-host binding, and actor-owned embedded
  public-key parsing. Exact remote account search and lookup can invoke it when
  a write pool is configured, enforce normalized domain allow/block policy,
  reuse fresh stored accounts, refresh stale WebFinger data, throttle
  process-local IP/handle misses, and transactionally upsert the minimal
  account identity by canonical actor URI. Same-origin redirects are rejected
  before cross-origin requests are made; actor field/key-count bounds,
   optional URL shapes, legacy `WHITELIST_MODE`, and current writer table/
   sequence capabilities are covered. Host-meta fallback and signed GETs now
   use origin-bound, redirect-re-signed transport. Actor public-key entries may
   be embedded or referenced; referenced key documents are bounded, fragment-
   safe, owner/ID checked, and reconciled into `keypairs` without discarding
   revocation or expiry metadata. A whole-resolution deadline and partial-key
   failure handling prevent key lists from multiplying latency. On-demand key
   refresh during inbound signature verification, media fetching, shared
   throttling, and adversarial federation transport fixtures remain open. Added
   bounded on-demand key refresh for inbound actor signatures: limited-mode and
   domain policy are checked before fetch, owner actor/WebFinger loopback is
   confirmed, stale keys retry once, fresh signatures verify before persistence,
   and incomplete key lists cannot delete existing key material. The full
   local, preflight, startup, and 14-case differential checks pass.
- Added the first ActivityPub inbox ingress boundary. Instance, shared,
  username, and numeric account inboxes now enforce an exact 1 MiB body bound,
  require remote HTTP signatures with signed SHA-256 digests, and enqueue opaque
  ingress jobs only after verification. A transactional 30-day idempotency
  marker deduplicates shared-inbox deliveries and retries, while ordering markers
  serialize jobs per signer. Web creates a runtime operational queue pool; the
  separate ActivityPub Note and relationship milestones will provide processing
  handlers without moving activity mutation or remote work into the web path.
  The local full gate passes 100 library tests plus all configured target tests,
  Clippy, formatting, cargo-deny, and fixture verification.
- Extended the bounded remote transport with signed ActivityPub JSON POSTs.
  Host, Date, Digest, and `(request-target)` coverage is rebuilt for every
  same-origin 307/308 redirect; request and response bodies remain bounded,
  compressed responses are rejected, and successful delivery statuses are
   accepted. The guarded worker integration now proves inbox deduplication and
   per-signer ordering against the restored fixture; HTTP-signature and inbox
   ingress issues are archived, while media fetching and activity processing
   remain separate open milestones.
 - Re-ran the restored-fixture worker integration and the complete 14-case
   Rails-versus-Rust differential suite after the transport changes. Both pass;
   the read-only REST surface now has production coverage for its listed routes
   and is archived alongside HTTP signatures and inbox ingress.
- Started outbound ActivityPub delivery with transactional local-status
   distribution intent. Push workers serialize Mastodon-compatible Create/Note
   activities, deduplicate remote shared inboxes into immutable per-inbox outbox
   events, enforce ActivityPub/domain policy and limited-federation mode, and
   deliver through signed bounded POSTs with retry/permanent classification and
   operational domain health. Private/direct audiences and reply targets are
   included; interaction, edit/delete, peer, and crash/retry delivery coverage
   remain open.
 - Expanded the durable ActivityPub relationship worker beyond Follow and Undo
   Follow. Remote Block and Undo Block now tear down relationships and preserve
   tombstone idempotency; blocked Follow requests produce signed Reject
   delivery intent; inbound Accept and Reject transition or remove URI-bound
   follow requests; and manual approval records a signed Accept outbox event in
   the same transaction as the local decision. Embedded and URI-only Undo,
   signer/domain checks, relationship locks, counters, and notification
   semantics remain in the write path.
 - Added restored-fixture worker coverage for blocked-follow rejection, Block
   and Undo Block, and inbound Accept/Reject transitions. The current checks
   pass: worker integration 10/10, schema integration 23/23, 115 library
   tests, all target tests, Clippy with warnings denied, formatting, and diff
   validation. Local relationship fan-out, actor Update/Delete, Note
   processing, and peer crash/order coverage remain open.
 - Added local-to-remote Follow/Undo delivery. Origin-aware relationship writes
   persist deterministic Follow URIs, enqueue signed shared-inbox delivery
   events atomically, cancel unsent or unleased Follow work when it is undone,
   and retain the original URI in the Undo payload. Delivery rechecks
   limited-federation and
   domain-block policy, while per-account domain blocks prevent local Follow
   creation. Restored-fixture schema coverage proves remote Follow/Undo payloads,
   duplicate outbox suppression, and cancellation behavior.
 - Tightened relationship convergence by removing FollowRequest notifications
   when inbound Accept/Reject decisions consume a request, adding remote-domain
   metadata to Accept/Reject delivery events, and avoiding notification writes
   for duplicate remote Follow activities. Local Block/Undo Block, Reject, and
   remove-follower delivery now use the same transactional signed outbox path;
   actor lifecycle, Note processing, and strict cross-worker ordering remain
   open.

## 2026-08-25

- Completed local-to-remote Block/Undo Block and Reject/remove-follower delivery
  with deterministic relationship URIs, duplicate-safe logical keys, unsent
  cancellation, remote-domain policy metadata, and signed per-inbox outbox
  events. Block teardown emits Undo Follow for removed outgoing relationships
  and Reject for removed incoming requests while retaining outgoing pending
  FollowRequests to match the existing Mastodon block behavior.
- Added a delivery-time state fence that suppresses stale queued positive Follow
  and Block activities after their relationship URI has been removed. Strict
  ordering across concurrent positive/undo deliveries remains a follow-up.
- The current verification gate passes: worker integration 13/13, schema
   integration 23/23, 119 Rust tests, all target tests, Clippy with warnings
   denied, formatting, cargo-deny, and diff validation. Outbound interaction
   fan-out and peer crash/order coverage remain open.
- Added signed actor Update/Delete handling. Updates validate that the embedded
   Actor is the verified signer, persist bounded profile/endpoints/field data,
   and ignore updates after deletion. Deletes sever relationships and related
   notifications, soft-delete remote statuses, clear the remote profile, and
   preserve idempotent tombstone-like suspension state. Restored-fixture worker
   coverage now passes 11/11, including update, delete, and non-resurrection.
- Added inbound Note Create/Update/Delete handling. Notes validate signer and
  object identity, preserve bounded raw HTML, audience visibility, replies,
  mentions, hashtags, remote media metadata, edit timestamps, and untrusted
  interaction counts, while URI locks, tombstones, duplicate suppression, and
  deleted-object fencing prevent resurrection. Restored-fixture worker coverage
  now passes 13/13, including duplicate Create/Delete, media metadata, and an
  Update after Delete.
- Added inbound Like/Undo and Announce/Undo handling. Interaction activities
  validate the remote actor/activity host, target only active local public or
  unlisted statuses, update favourite/reblog counters transactionally, create
  notifications, and use activity tombstones to fence duplicate and
  Undo-before-activity delivery. Media fetching, unresolved-parent reply
  repair, outbound interaction fan-out, and peer differential/order coverage
  remain open.
- Completed durable out-of-order ActivityPub reply repair against the pinned
  Mastodon `ThreadResolveWorker` behavior. Unresolved Note children remain
  `reply = true` and record one transactional Pull-lane job; resolution first
  checks exact and computed local status URIs, then performs bounded same-origin
  signed parent fetches, accepts Note/Create representations, validates object
  identity, resolves/upserts unknown parent actors, and attaches the child with
  Mastodon's carried reply-account rule and exactly-once reply counters. Inbound
  Notes now apply the Mastodon local-relevance gate, scalar/null `to` and `cc`
  audiences are accepted, and default write workers consume Pull jobs. The
  restored fixture passes 14/14, including remote child-first and computed-local
  parent cases. Media bytes, adversarial transport fixtures, and the full peer
  differential/order matrix remain open.
- Added a durable Pull-lane remote media job that uses the bounded SSRF-safe
  fetcher, enforces a 16 MiB response limit, applies the remote account's
  domain/reject-media policy, preserves Note text on failure, retries transient
  failures, and stores supported images through the exact Paperclip cache paths.
- Added tolerant Mastodon attachment-link parsing, description truncation,
  focal-point metadata, blurhash validation, media proxy retrieval, and
  Paperclip recovery for pre-existing partial files. The proxy now applies the
  existing REST status policy before fetching public or authenticated media and
  accepts Mastodon's no-style and trailing-style route forms.
- Restored-fixture worker integration now passes 15/15, including the media
  failure case; successful remote HTTP media fixtures, media lifecycle cleanup,
  adversarial transport, and the full peer differential/order matrix remain
  open.
- Moved local status mention, favourite, and reblog notification creation into
  transactional Core-lane outbox jobs. The worker resolves mentions and
  interactions through the existing idempotent policy kernel; restored-fixture
  coverage now proves 16/16 worker cases and 24/24 schema cases, including
  duplicate notification delivery and mention dispatch.
- Added origin-aware local-to-remote `Like`, `Announce`, `Undo Like`, and
  `Undo Announce` delivery. Payload builders match Mastodon's nested activity
  shapes, deterministic actor/status IDs, public/unlisted/private audiences,
  remote-domain policy, and unsent positive-event cancellation. Schema coverage
  proves the outbound interaction lifecycle; actor edits/deletes, media success
  fixtures, crash/order delivery, and peer differential coverage remain open.
- Added a deterministic feature-gated HTTP media fixture without relaxing
  production SSRF checks. The real media worker path now proves bounded fetch,
  GIF processing, cached Paperclip original storage, PNG small-style storage,
  metadata, and blurhash persistence; restored-fixture worker coverage passes
  17/17. Adversarial transport, cleanup/crash compensation, and peer
  differential coverage remain open.
- Added durable local status Update and Delete distribution. Edits emit
   Mastodon-compatible `#updates/<unix-seconds>` activities; deletes emit
   `#delete` Tombstones and are allowed through delivery after the status is
   soft-deleted. Distinct logical keys prevent edits/deletes from colliding with
   Create delivery, while deletion cancels only pending obsolete work. Worker
   coverage now passes 18/18 and schema coverage proves transactional Update and
   Delete outbox intent; actual peer delivery, crash/order behavior, and actor
   Update/Delete remain open.
 - Hardened status delivery after independent review: Update keys include the
   activity version, stale queued edits are coalesced before fan-out and fenced
   again before delivery, and delivery-time domain policy is rechecked even for
  older jobs without stored domain metadata. Status reach now follows
  Mastodon’s public/unlisted interaction rules for replies, reblogs, quotes,
  favourites, parent authors, and quote targets without expanding private,
  direct, or limited audiences. The feature-gated media fixture endpoint now
  refuses release builds; the aggregate suite passes 126 library tests,
   worker integration 18/18, schema integration 24/24, formatting, Clippy,
   cargo-deny, fixture verification, and diff validation.
 - Added real bounded-transport coverage for remote fetching. A local HTTP
   fixture now proves oversized responses, cross-origin redirects, and stalled
   responses fail closed through the production fetch implementation; existing
   DNS-address and canonical-identity tests cover rebinding and representation
   attacks without permitting internal network access.
  - Added a debug-only local inbox endpoint for deterministic signed ActivityPub
    POST tests. The durable worker now proves Host, Date, Digest,
    `(request-target)`, RSA signature, body delivery, and successful 2xx response
    handling; restored-fixture worker coverage passes 19/19 without changing the
    production DNS/address policy.
  - Moved follow, follow-request, inbound Note mention, Like, and Announce
    notification production into transactional Core-lane outbox events. The
    web and ActivityPub ingress paths now only mutate relationship/content state;
    the notification worker performs the idempotent policy-kernel write. Worker
    integration remains green at 19/19, including local FollowRequest dispatch
    and inbound interaction notification delivery.
  - Closed notification lifecycle races found in review. Deletion now uses the
    recipient advisory lock, cancels pending notification outbox events, and
    reconciles filtered notification requests. Already-dispatched jobs remain
    runtime-owned and are fenced by activity re-resolution. Local and remote
    mention edits preserve withdrawn mentions as silent rows, reactivate existing
    rows on re-mention, and cancel undelivered stale jobs instead of deleting history.
  - Kept the notification lifecycle compatible with the least-privilege web
    writer: the fixture grants only column-scoped UPDATE on mention silence and
    timestamps, preflight validates those capabilities, and the writer never
    mutates runtime-owned durable jobs. The focused notification differential now
    passes again after the unfollow/status-mention privilege regressions.
  - Added transactional outbound actor `Update` delivery for local profile
    changes. Versioned Push jobs serialize profile media and property fields,
    reach remote followers/reporters/recent contacts/enabled relays, deduplicate
    inboxes, and fence stale versions before creating signed delivery events.
   Restored-fixture worker coverage is now 20/20; schema and startup integration
   remain green.
 - Added local `POST /api/v1/reports` with `write:reports` authorization,
   transactional report/status/collection persistence, Rails-compatible category
   and omitted-forward handling, REST serialization, and durable `admin.report`
   notifications. Restored-fixture schema coverage passes 25/25, worker coverage
   passes 20/20, and the differential report case proves valid attached reports,
   silent-mention attachment, response parity, invalid-collection rollback, and
   invalid rule handling. Rule reads are granted to the least-privilege writer;
   target availability and duplicate rule validation now match the pinned peer.
   The generated URI intentionally follows the fixture's observed
   `<origin>/<uuid>` result from Mastodon's `URI.join` behavior.
 - Added permission-checked `admin resolve-report` and `--reopen` commands.
   Resolution updates and report audit rows commit together, reject disabled or
   non-moderating accounts, and are covered by the restored schema test and CLI
   help coverage.
 - Added permission-checked `admin delete-status`, reusing the local status
   deletion transaction for counters, reblogs, media behavior, tombstone
   delivery, and optional Paperclip cleanup; moderation deletions are audited.
  - Added administrator-only `admin reconcile-account-stats`, which repairs the
    denormalized account counters and latest-status timestamp transactionally and
    records the repair in the moderation audit log.
  - Added durable local boost distribution. Reblogs now enqueue Push-lane
     Announce work, unboosts cancel pending distribution and enqueue Undo Announce,
     original-status deletion preserves remote reblogger recipients, public status
     delivery reaches enabled relays, and private self-boosts inline the original
     Note. Mastodon fixture worker/schema integration passes 26/26 and 30/30.
  - Closed deletion fan-out and teardown gaps found in the federation review.
    Delete stream events now remove public, unlisted, and private entries from
    all active local followers while retaining direct/limited audience bounds;
    remote Note and actor deletion removes affected favourites, polls, and
     Favourite/Poll notifications with counter restoration. Worker and schema
     integration remain green at 26/26 and 30/30.
   - Added Mastodon-compatible local status activity routes at username and
     numeric account paths. Public, private, and direct `Create`/`Announce`
     responses now use verified ActivityPub or OAuth authorization; guarded
    differential coverage passes 18/18, including anonymous denial and signed
    private/direct reads.
  - Completed the local account deletion purge slice: self-service deletion now
    queues a durable 30-day maintenance purge, retains the actor identity and
    reported content, removes account-owned content and interactions
    transactionally, and preserves signed actor Delete delivery. Restored-fixture
    worker coverage passes 29/29, Mastodon schema coverage 30/30, and rollback,
    retry, poll/bookmark/pin retention, unsuspend cancellation, writer privilege,
     preflight, startup, and operational-schema checks pass.
   - Hardened outbound ActivityPub protocol boundaries: remote status mentions,
     status/account fan-out, report forwarding, remote reply forwarding, and
     follow acceptance now exclude protocol-0 accounts. Account-update delivery
     also fences microsecond versions against second-resolution activity IDs.
      Focused Rust tests pass; restored-fixture regressions and live peer
      convergence remain pending.
  - Added token-scoped authenticated-stream revocation. OAuth revocation,
    password resets, and browser-session deletion now record durable
    `kill:token` events; only the matching WebSocket closes, while account-wide
    suspension/deletion kills remain unchanged. The restored fixture proves
    revocation, reconnect rejection, sibling-token continuity, and cleanup
    without changing the fixture sequence.
  - Preserved OAuth consent requests across browser login with a validated
    local return target, added Rails-compatible CORS for OAuth/discovery
    surfaces, and switched OAuth client-secret comparisons to constant-time
    checks. Full local checks, the OAuth differential case, and operational
    streaming integration pass.

## 2026-09-03

- Hardened account purge filesystem cleanup. Claimed purge jobs now persist
  validated Paperclip paths before the database transaction, commit database
  cleanup first, and remove files afterward; failures retain the manifest for
  a lease-fenced retry. Restored-fixture coverage forces the filesystem failure
  after commit and proves recovery, while the durable queue test rejects stale
  manifest updates. Worker integration passes 35/35; hard power-loss and
  production disk-failure behavior remain unproved.
                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  
