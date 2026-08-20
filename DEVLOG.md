# Development Log

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
