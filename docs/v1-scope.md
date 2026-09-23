# Rustodon v1 Scope

## Goal

Rustodon v1 is a mostly-in-place replacement for a small, self-hosted Mastodon
instance. An operator should be able to stop Mastodon, point Rustodon at the
same PostgreSQL database and local media directory, retain the same domain and
secrets, and continue using existing accounts, posts, relationships, media,
OAuth clients, and federation identities.

The goal is persisted-data, client, and federation compatibility for normal
use. It is not a bug-for-bug Rails rewrite, a cache migration, or immediate
coverage of every optional Mastodon feature.

## First Compatibility Target

Target one stable Mastodon release before supporting a version range. The
checked-out `main` branch is 4.7.0-beta.1 (`mastodon/lib/mastodon/version.rb`),
while its production Compose file still pins 4.6.5. Rustodon should target the
4.6.5 release and schema first, then add 4.7 as a separate compatibility
target. Developing against moving `main` would make the first milestone chase
further quote, collection, keypair, and schema changes instead of a fixed
contract. Mastodon 4.6.5 already contains quote, collection, collection-item,
and keypair data that Rustodon must read and preserve.

Rustodon must inspect `schema_migrations` at startup and refuse unknown schema
versions. Schema adapters should be explicit rather than a collection of
best-effort column checks.

## V1 Deployment Assumptions

- One domain and a small instance with 1-20 existing local users.
- PostgreSQL is the authoritative durable data store.
- Uploaded files use Mastodon's local Paperclip filesystem layout.
- S3, Swift, Azure, and other object storage are not enabled.
- Registration is closed. Existing users can log in; an administrator can add
  a user with a CLI command if needed.
- Existing Mastodon Sidekiq work is drained before cutover.
- Mastodon and Rustodon do not concurrently write the database.
- Redis data may be discarded. Rustodon computes timelines from PostgreSQL and
  uses its own durable job mechanism.
- Elasticsearch and optional external integrations are disabled.
- Existing Rails browser sessions may expire at cutover. Existing OAuth bearer
  tokens remain valid.

## Compatibility Contract

### Must Be Preserved Exactly

- `LOCAL_DOMAIN`, `WEB_DOMAIN`, canonical actor URLs, status URLs, and object
  URIs.
- PostgreSQL IDs. Rustodon must let PostgreSQL's existing `timestamp_id()`
  defaults allocate timestamp IDs with `INSERT ... RETURNING id`.
- Existing users, bcrypt password hashes, OAuth applications, access tokens,
  scopes, roles, and account state.
- Existing local ActivityPub RSA keys. Rustodon must support the key layout and
  encryption format of each declared schema target.
- PostgreSQL enum integer values, polymorphic type strings, arrays, JSON/YAML
  formats, NULL-versus-empty behavior, and soft deletion.
- Local media files, Paperclip paths, filenames, styles, and database metadata.
- Visibility and authorization semantics for public, unlisted, followers-only,
  limited, and direct statuses.
- Existing follows, follow requests, blocks, mutes, favourites, bookmarks,
  mentions, conversations, notifications, filters, and moderation blocks.
- Existing tombstones and deleted status state.

### May Be Rebuilt or Replaced

- Redis caches.
- Redis home and list feeds.
- Redis Pub/Sub and presence keys.
- Sidekiq's queue representation after all old queues are drained.
- Trends, recommendations, activity counters, and search indexes.
- Rendered JSON and HTML caches.

### Rustodon-Owned Database Objects

Adding a small, separate `rustodon` PostgreSQL schema is acceptable and does
not require migrating user data. It should contain only operational state such
as:

- Durable jobs with lanes, retries, leases, and `run_at`.
- Transactional outbox events.
- API idempotency keys.
- Expiring protocol-ordering markers.
- Per-domain delivery health.
- Worker and scheduler heartbeats.

Do not change Mastodon-owned tables merely to make the Rust implementation
more convenient.

## V1 Features

### 1. Startup, Safety, and Cutover

- Parse the common Mastodon environment variables for domains, PostgreSQL,
  local media, SMTP, trusted proxies, and encryption secrets.
- Verify the exact supported schema version, required database function and
  sequences, local account keys, writable media root, and configured domains.
- Provide a `rustodon preflight` command that reports unsupported active data
  or configuration before the maintenance window.
- Refuse startup for object storage, unknown schema versions, unreadable local
  keys, pending unsupported destructive jobs, or an unsafe media layout.
- Provide `/health` and worker/readiness checks.
- Preserve the existing Nginx-facing paths and support forwarded headers only
  from configured trusted proxies.
- Document a snapshot, drain, cutover, smoke-test, and rollback procedure.

Preflight should specifically detect pending scheduled statuses, pending account
deletions, enabled WebAuthn-only users, non-empty Sidekiq queues, active relays,
object storage, and enabled SSO providers. The first version may ask the
operator to resolve these before cutover rather than implement them badly.

### 2. Authentication and OAuth

- Accept existing OAuth bearer tokens and enforce revocation, expiry, owner
  state, and broad or granular scopes.
- Password login and logout for existing users.
- Existing TOTP and backup-code verification. WebAuthn is not required.
- OAuth authorization-code flow with PKCE and token revocation.
- Dynamic client registration through `POST /api/v1/apps` and application
  credential verification. Mobile clients depend on this.
- Basic password reset and confirmation email when SMTP is configured, plus an
  administrator CLI reset path.
- A minimal browser session format owned by Rustodon. Rails session-cookie
  compatibility is not required.

Open registration, invites, CAPTCHA, LDAP, PAM, CAS, SAML, OIDC, and arbitrary
OmniAuth providers are outside v1.

### 3. Web Client

- Reuse static frontend assets built from the exact supported Mastodon release
  rather than writing a new social UI in v1.
- Render the expected HTML shell, Vite manifest entries, `initial-state` JSON,
  CSRF data, and `#mastodon` mount point.
- Serve the matching static assets, themes, locale chunks, `/sw.js`, and
  existing uploaded files.
- Implement a small Rust-owned login, password-reset, and essential account
  settings UI. Recreating all Rails/HAML settings and admin pages is not a v1
  requirement.
- Advertise unsupported optional features as disabled and return stable empty
  responses where the frontend probes them.

The frontend contract is visible in
`mastodon/app/views/shared/_web_app.html.haml`,
`mastodon/app/serializers/initial_state_serializer.rb`, and
`mastodon/app/javascript/mastodon/features/ui/index.jsx`.

### 4. Core Mastodon REST API

Implement normal Mastodon JSON conventions: decimal string IDs, ISO-8601
timestamps, rendered HTML content, fixed nullable keys, `Link` pagination, CORS,
scope checks, and conventional error status codes.

Required instance and identity endpoints:

- `GET /api/v1/instance`
- `GET /api/v2/instance`
- `GET /api/v1/instance/rules`
- `GET /api/v1/instance/translation_languages` returning `{}` when disabled
- `GET /api/v1/accounts/:id`
- `GET /api/v1/accounts/verify_credentials`
- `PATCH /api/v1/accounts/update_credentials` for text/profile preferences
- `GET /api/v1/accounts/lookup`
- `GET /api/v1/accounts/search`, including exact remote-handle resolution
- `GET /api/v1/accounts/relationships`
- Account statuses, followers, and following lists

Required status endpoints:

- Create, show, edit, and delete status.
- Status context, source, and edit history.
- Quote fields in status responses; quote creation through `quoted_status_id`,
  quote-policy updates, quote lists, and local revocation with visibility/block/
  policy enforcement and atomic failure semantics.
- Existing tagged-collection fields in status responses without enabling their
  creation workflows.
- Public, unlisted, followers-only, limited, and direct visibility.
- Replies, content warnings, sensitive flag, language, mentions, and up to four
  image attachments.
- Favourite/unfavourite, bookmark/unbookmark, boost/unboost.
- Reblogged-by and favourited-by lists.
- API idempotency for status creation.

Required timelines and collections:

- Home timeline computed from PostgreSQL with follow, reply, language,
  visibility, block, mute, and exclusive-list filtering.
- Local/public timeline.
- Hashtag timeline.
- Account timeline.
- Favourites, bookmarks, blocks, and mutes lists.
- Follow, unfollow, follow-request list/accept/reject.
- Block/unblock and mute/unmute.
- Conversations list, read/unread, and remove from inbox.
- Existing custom filters must be applied and returned as status annotations.
- Markers for home and notifications.

Required notifications:

- Persist mention, follow, follow-request, favourite, boost, status-update, and
  moderation-warning notifications.
- Read and serialize every notification type stored by Mastodon 4.6.5,
  including poll, severed relationship, annual report, admin, quote,
  quoted-update, added-to-collection, and collection-update notifications,
  even when Rustodon v1 does not create the underlying optional workflow.
- v1 notification list/show/dismiss/clear/unread-count endpoints.
- v2 grouped notification list/show/dismiss/clear/unread-count for the current
  Mastodon web client.
- Enforce existing blocks, mutes, conversation mutes, and notification policy
  rows. Email and Web Push side effects are separate and optional.

Required media behavior:

- Serve every existing local image, video, audio, avatar, header, emoji, and
  preview file referenced by the database.
- Upload and process new images, avatars, and headers into Paperclip-compatible
  paths and metadata.
- Implement v1/v2 media create, show, update, and delete semantics.
- Preserve descriptions/alt text and image dimensions.
- Publish local media metadata only after fsynced files exist, and retain durable,
  idempotent cleanup work across creation, deletion, and process-crash boundaries.
- New audio/video transcoding is not required, but existing processed audio and
  video remain readable.

The full route inventory is in `mastodon/config/routes/api.rb`; v1 should expose
only the subset above plus harmless empty endpoints required at frontend
startup, rather than claiming all routes are implemented.

### 5. Federation

Required discovery and representations:

- WebFinger and host-meta.
- NodeInfo discovery and NodeInfo 2.0.
- Local actor JSON-LD with stable IDs, inbox, outbox, followers/following,
  shared inbox, and legacy RSA public key.
- Local Note object GET, its `replies`/`likes`/`shares` collections, and basic
  paginated outbox/followers/following `OrderedCollection` responses.

Required transport and fetching:

- Draft Cavage RSA-SHA256 HTTP signature generation and verification.
- SHA-256 body digest verification, clock-skew limits, and exact-body handling.
- Signed ActivityPub POSTs and signed GET support.
- Per-user and shared inboxes with a 1 MiB limit, synchronous authentication,
  durable enqueue, and `202 Accepted`.
- Remote WebFinger, actor, key, and status fetch with strict canonical-ID,
  origin, content-type, redirect, response-size, timeout, and SSRF checks.
- URI-only `Create Note` objects are resolved by durable signed pull work rather
  than being acknowledged and dropped.
- Shared-inbox deduplication, durable outbound delivery, retries/backoff,
  permanent-error classification, and domain health tracking.

Required activity handling:

- `Follow`, `Accept`, `Reject`, and `Undo Follow`.
- `Create`, `Update`, and `Delete` for `Note`.
- `Like` and `Undo Like`.
- `Announce` and `Undo Announce`.
- `Block` and `Undo Block`.
- Actor `Update` and `Delete`.
- `QuoteRequest`, quote `Accept`/`Reject`, `QuoteAuthorization`, authorization
  `Delete`, target-owner-signed scalar instrument fetch, quoted-Note
  updates/Tombstone removal, final send-time lifecycle fencing, and signed
  authorization forwarding.
- Replies, mentions, public/unlisted/followers/direct audience handling, and
  tombstones that prevent deleted objects from reappearing.
- Image attachment metadata and bounded remote image caching. Text remains
  usable if a media download fails.
- Domain-bound custom emoji metadata, bounded image fetching/decoding, status and
  poll association, and outbound Emoji tags/resources.

The minimum is intentionally the old, widely compatible federation profile.
Mastodon itself identifies WebFinger and HTTP signatures as required extensions
in `mastodon/FEDERATION.md`.

### 6. Durable Work

Rustodon needs worker and scheduler modes even for a tiny instance. The web
process should never perform ordinary federation delivery or remote media work
inline.

Required job lanes:

- Ingress: authenticated inbox processing.
- Core: local distribution, notifications, and post-commit work.
- Push: outbound ActivityPub delivery.
- Pull: bounded remote actor/status/media fetching.
- Mail: authentication/security email.
- Maintenance: cleanup and reconciliation.

Required semantics:

- Transactional enqueue with database writes.
- At-least-once execution and idempotent handlers.
- Leases with recovery after worker crashes.
- Delayed jobs and cancellation by logical key.
- Retry limits and jittered backoff.
- Dead-letter inspection.
- Separate concurrency limits for remote HTTP and media work.
- SMTP effects are bounded at-least-once: acceptance before durable acknowledgement
  can repeat, retries preserve one opaque `Message-ID`, and exhausted attempts are
  inspectable dead letters rather than an exactly-once guarantee.

### 7. Timelines and Streaming

- Build home, list-related filtering needed by home, public, and tag timelines
  from PostgreSQL. Do not reproduce Redis feed caches in v1.
- Implement the authenticated `user` WebSocket stream used by the first-party
  web client, including status update/delete, notifications, and conversations.
- Implement token revocation/connection termination.
- Public, list, hashtag, and SSE streams may follow after the core user stream.
- Polling remains a valid fallback and all durable state lives in PostgreSQL.

### 8. Safety and Basic Moderation

- Enforce existing account blocks, user domain blocks, global suspended-domain
  blocks, suspended/silenced/deleted account state, and local role permissions.
- Support report submission.
- Provide administrator CLI commands to inspect/resolve reports, suspend or
  unsuspend an account, block or unblock a domain, and delete a local status.
- Preserve audit/history tables even when Rustodon does not expose their full
  UI.
- Never hard-delete an ordinarily deleted or reported status as the first
  action. Preserve Mastodon's soft-delete/tombstone behavior and federation
  deletion side effects.
- Keep login, federation, and remote-fetch abuse limits. Exact Rails rate-limit
  bucket behavior is not required.

## Read and Preserve, But Do Not Create in V1

Rustodon must tolerate these existing rows and serialize them where they occur,
but it does not need to expose their mutation workflows:

- Existing list memberships, pins, featured tags, and followed tags.
- Existing custom filters and explicit status filters.
- Existing scheduled-status records, provided none are pending at cutover.
- Preview cards and custom emoji.
- Existing announcements.
- Existing reports, warnings, appeals, severance records, account migrations,
  imports, backups, and web-push subscriptions.
- Existing Mastodon 4.6.5 collection, collection-item, and keypair rows. They
  must be mapped and serialized where externally visible, but v1 does not need
  to create or mutate collection workflows.
- Unknown notification types and future enum values. Preserve them; do not
  rewrite them into a known type.

All unsupported Mastodon-owned tables stay untouched so rollback and later
feature work remain possible.

## Explicitly Out of V1

### Storage and Infrastructure

- S3, Swift, Azure, and other object stores.
- CDN invalidation and remote cache-purge APIs.
- Elasticsearch and full-text status search.
- Redis compatibility or Sidekiq queue consumption.
- Read replicas, PgBouncer-specific optimizations, multi-region operation, and
  high-availability queue behavior.
- Prometheus/OpenTelemetry parity beyond basic Rust metrics and health.

### Authentication and Registration

- Open registration and API account registration.
- Invites, approval queues, CAPTCHA, date-of-birth policy, and email-domain
  registration rules.
- WebAuthn/security keys.
- LDAP, PAM, CAS, SAML, OIDC, and OmniAuth providers.
- Rails browser-cookie compatibility.

### User Features

- Scheduled posts.
- Custom/featured collections.
- New audio/video/GIF transcoding.
- List CRUD and membership mutation.
- Filter CRUD, tag follows, featured tags, endorsements, and account notes.
- Pinned posts.
- Link previews, oEmbed generation, and translation.
- Push notifications and profile email subscriptions.
- Imports, full archive backups, account migration, and aliases.
- Announcements, trends, recommendations, directory, annual reports, donation
  campaigns, and advanced search.
- RSS feeds and advanced embed pages.

### Federation Extensions

- Relays and generic shared-inbox forwarding.
- Featured-collection federation.
- Follower synchronization.
- Account `Move` migration.
- Federated `Flag` reports.
- Authorized-fetch mode as an instance policy. Signed GET support is still in
  v1 for interoperability.
- RFC 9421 signatures, Ed25519/Multikey, Linked Data Signatures, Object
  Integrity Proofs, ML-DSA, and bearcaps.
- Recursive replies collection crawling.
- Group actor semantics and FASP.

### Administration

- Full Mastodon admin dashboard and analytics.
- Granular moderation workflows, assignment, appeals UI, batch moderation,
  trend review, and federated report forwarding.
- Automated remote-content retention and per-user status-cleanup policies.
- Bulk email and webhooks.

## Non-Obvious Data Invariants

Every native write must preserve the complete operation, not only its obvious
row:

- Status creation also maintains URI, conversation, mentions, tags, media
  ordering, `account_stats`, `status_stats`, notifications, and durable
  federation/distribution jobs.
- Status deletion is a soft discard plus boost removal, counter updates,
  conversation cleanup, tombstone/federation work, and later hard cleanup.
- A follow adjusts both account counters; a remote follow stays pending until
  an `Accept` is received.
- A favourite or bookmark targets the original status, not a boost wrapper.
- A boost needs serialization/locking because Mastodon lacks a database unique
  constraint for `(account_id, reblog_of_id)`.
- Direct messages are direct-visibility statuses plus mention-based access and
  sorted `account_conversations` arrays with optimistic locking.
- Removed mentions on edits may need to become silent mentions so previously
  granted access is not revoked accidentally.
- Poll tallies and conversation arrays use optimistic locking.
- Notification polymorphic class strings and semantic type strings are stored
  contracts.
- `accounts.domain IS NULL` means local, but not every local account is a
  login-capable user; the instance actor is a special negative ID.
- NULL, empty string, and empty PostgreSQL array are not interchangeable.

These invariants are spread across models and services, especially
`mastodon/app/services/post_status_service.rb`,
`mastodon/app/services/remove_status_service.rb`,
`mastodon/app/services/follow_service.rb`,
`mastodon/app/services/notify_service.rb`, and the callbacks in
`mastodon/app/models/status.rb`.

Paths prefixed with `mastodon/` refer to the separately obtained, pinned
Mastodon source checkout. They are not files vendored into this repository.

## Implementation Order

### Phase 0: Compatibility Harness

- Check out the exact Mastodon 4.6.5 tag alongside Rustodon.
- Build repeatable PostgreSQL and local-media fixtures from Mastodon factories.
- Add schema/version detection and read-only model mappings.
- Add Rails-versus-Rust golden response and database-diff runners.
- Produce cryptographic test vectors for password/TOTP/key handling and HTTP
  signatures.

### Phase 1: Read-Only Local Instance

- Configuration, health, schema guard, media serving.
- Existing OAuth token authentication.
- Account, status, quote, collection, relationship, instance, notification,
  and filter serialization.
- Visibility-correct account and status reads.
- Public, account, tag, and home timeline reads with visibility filtering,
  including exclusive-list behavior.
- WebFinger, actor, status object, NodeInfo, and signed GET responses.

At this point Rustodon can be exercised against a cloned real database without
being allowed to mutate it.

### Phase 2: Federation Transport

- RSA key loading and Rails-compatible decryption for the target release.
- HTTP signature verification/signing.
- SSRF-safe remote fetcher.
- PostgreSQL job queue and transactional outbox.
- Inbox authentication/enqueue and outbound delivery/retry.
- Remote account and status ingestion.

### Phase 3: Core Local Writes

- Status create/edit/delete, replies, mentions, content warnings, and image
  attachments.
- Home/public/tag distribution and notifications.
- Follow lifecycle, favourite, bookmark, boost, block, and mute.
- Create/Update/Delete, Follow, Like, Announce, Block, and Undo federation.
- Counter and denormalized-data reconciliation commands.

### Phase 4: User-Facing Cutover

- Password/TOTP login, OAuth authorization, and dynamic app registration.
- Matching Mastodon frontend assets, HTML shell, and initial state.
- Grouped notifications, markers, conversations, and authenticated user
  streaming.
- Essential profile/settings pages and authentication email.
- Preflight, cutover, rollback, and smoke-test tooling.

### Phase 5: Hardening and Release

- Differential tests for every supported endpoint and command.
- Federation tests against a real Mastodon peer and malformed/adversarial
  fixtures.
- Crash/retry/idempotency tests for every job.
- Authorization matrix and private/direct-content leak tests.
- Media path/metadata compatibility tests and rollback into Mastodon.
- Load, timeout, queue saturation, disk-full, and database-failure tests.

## Test-Porting Strategy

A test-by-test port is a good mechanism after the v1 boundary is fixed, but do
not mechanically port every Rails unit test. Port externally observable
contracts and state transitions; write native Rust tests for implementation
details.

### Port First

1. Schema, enum, serialization, ID, visibility, and authorization tests.
2. REST request specs for instance, apps/OAuth, accounts, statuses, timelines,
   media, notifications, follows, and social interactions.
3. WebFinger, actor/status JSON-LD, inbox/outbox, signature, and ActivityPub
   parser/activity tests.
4. Service behavior for post, edit, delete, follow, favourite, reblog, block,
   mute, notify, and remote resolve/fetch.
5. Worker tests for ingress, delivery retry, distribution, and idempotency.

Useful source suites include:

- `mastodon/spec/requests/api/v1/`
- `mastodon/spec/requests/api/v2/`
- `mastodon/spec/requests/activitypub/`
- `mastodon/spec/requests/well_known/`
- `mastodon/spec/requests/oauth/`
- `mastodon/spec/serializers/rest/`
- `mastodon/spec/serializers/activitypub/`
- `mastodon/spec/lib/activitypub/`
- `mastodon/spec/services/`
- `mastodon/spec/workers/activitypub/`

### Differential Test Shape

For reads:

1. Load the same PostgreSQL fixture into two databases.
2. Send the same HTTP request to Mastodon and Rustodon.
3. Compare status, relevant headers, and canonicalized JSON.
4. Normalize only known nondeterminism such as request IDs and timestamps.

For writes:

1. Clone one database and media fixture twice.
2. Execute the operation once through Mastodon and once through Rustodon.
3. Diff all affected Mastodon tables, files, emitted ActivityPub JSON, and
   durable jobs.
4. Repeat the request to prove idempotency.
5. Reopen both results through Mastodon to prove rollback compatibility.

Do not require matching SQL order, callback count, cache keys, job class names,
or exact Rails error prose unless a real client relies on it.

### Do Not Port in V1

- Tests solely for excluded routes and features.
- ActiveRecord callback wiring tests when the resulting transaction is already
  covered.
- Redis cache/feed internal representations.
- Sidekiq JSON, queue names, middleware, and retry internals.
- HAML/view-helper details outside the reused frontend shell.
- Exact query counts and Rails exception-class behavior.
- Exotic provider and object-storage test matrices.

## V1 Acceptance Criteria

The requirement-to-proof traceability table is maintained in the [v1 acceptance
matrix](v1-acceptance-matrix.md). It is intentionally explicit about local
automated evidence and acceptance work that still requires a browser, mobile
client, live peer, or cutover rehearsal.

Rustodon v1 is ready when tested against a one-domain, local-filesystem
Mastodon 4.6.5 instance with 1-20 local users:

- Preflight passes and no user-data transformation is required.
- The operator can stop Mastodon, drain Sidekiq, start Rustodon, and retain the
  same PostgreSQL database, local media tree, domain, and secrets.
- Existing users can log in with password and TOTP and existing OAuth clients
  remain authorized.
- The matching Mastodon web frontend and a mobile client version recorded in
  Rustodon's compatibility manifest can read and publish normal statuses,
  replies, images, follows, favourites, bookmarks, boosts, blocks, mutes,
  direct messages, and notifications.
- Existing public/private/direct content remains visible only to the correct
  viewers.
- A separately started peer pinned to Mastodon 4.6.5 can discover, follow,
  receive, reply to, like, boost, update, and delete content in both directions.
- Worker crashes and duplicate deliveries do not duplicate posts,
  relationships, or notifications.
- Existing media URLs continue to work and newly uploaded images remain
  readable after rolling back to Mastodon.
- Unsupported active configurations are rejected by preflight instead of being
  silently ignored.
- Redis can be removed after cutover, and restoring Mastodon remains possible
  without reversing a user-data migration.

### Instance activity API semantics

The v2 instance `usage.users.active_month` is the exact distinct membership union
for UTC `[today - 28 days, today)`. NodeInfo 2.0 `usage.users.activeMonth` and
`activeHalfyear` use `[today - 28 days, today)` and `[today - 168 days, today)`.
Only buckets whose expiry is strictly after the captured aggregation time count;
there is no join against current user approval, confirmation, disabled, or deleted
state. Limited federation suppresses only the v2/initial-instance month value.
NodeInfo deliberately retains both raw counts.

A shared per-WebState singleflight cache limits aggregation to one successful
refresh per 60 seconds, invalidates its entry on UTC rollover, and coalesces failures for
5 seconds. A refresh failure serves the last good counts for up to 24 hours,
otherwise zeroes; metadata-bearing responses (including the frontend document)
stay available. Manifest and instance-rules responses do not load activity
counts. The aggregation
SELECT has a 3-second statement timeout
inside a 5-second refresh deadline. Startup does not seed permanent zeroes.
Initial frontend `instance` metadata is serialized using the same v2 projection;
the pinned sidebar itself fetches `/api/v2/instance`.

See README's rollout notes: no backfill, today's records excluded, first ordinary
nonzero result after UTC rollover. Browser rendering evidence is separate from
API and initial-document integration evidence.
