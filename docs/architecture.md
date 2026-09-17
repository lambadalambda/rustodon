# Architecture and capabilities

## Compatibility goal

Rustodon is intended to replace Mastodon application processes while retaining
the installation's PostgreSQL database, local Paperclip media tree, domain,
canonical URLs, identifiers, OAuth identity, and ActivityPub signing keys. The
initial target is Mastodon 4.6.5 at commit
`1440d55b139e39ec722c2a3db7f60b66cd889048`, schema version
`20260611150940`.

Compatibility is defined by persisted data and externally observable REST,
browser, media, and federation behavior. Rustodon does not reproduce Rails
callbacks, ORM structure, SQL ordering, Redis keys, Sidekiq payloads, or
replaceable cache state.

The supported deployment is a small, closed-registration instance using one
PostgreSQL database and local filesystem media. Mastodon and Rustodon writers
must never run concurrently. See the [v1 scope](v1-scope.md) for the complete
boundary and [cutover runbook](cutover.md) for operational requirements.

## Data ownership

### Mastodon-owned state

Rustodon reads and updates the existing Mastodon schema through typed repository
boundaries. It keeps table layouts, foreign keys, IDs, canonical URLs, key
material, and unsupported historical rows compatible with Mastodon rollback.
Unknown enum strings, integer values, and permission bits are represented by
open wrappers instead of being discarded. IDs remain signed `i64` values.

`Repository` is the read boundary. Its SQLx pool is private, read connections
default to UTC and read-only operation, normal status reads exclude soft-deleted
rows, and it exposes no generic ORM, callback, or save API.

`WriteRepository` is the write boundary. It uses a separately configured,
least-privilege writer pool and explicit transactions. Without
`WRITE_DATABASE_URL`, the web process remains read-only against Mastodon-owned
data.

### Rustodon-owned state

Rustodon uses a separate `rustodon` schema for operational state. It contains a
schema version plus eight tables covering:

- durable jobs;
- transactional outbox events;
- idempotency keys;
- ordering markers;
- remote-domain health;
- process heartbeats;
- shared rate-limit windows;
- remote-fetch leases.

The operational schema is created only by
`rustodon admin migrate-operational-schema`; web and worker startup never perform
implicit DDL. A dedicated migration role owns the schema, while runtime and
writer roles receive only the grants required by their process boundaries.

Redis-derived feeds, queues, locks, and caches are rebuilt or replaced. Redis is
used only during preflight to verify that legacy Sidekiq work has drained and may
be removed after the rollback window.

## Process model

### Web

The web process serves REST APIs, browser authentication/settings pages, the
pinned Mastodon frontend, local media, federation endpoints, and authenticated
user streaming. It validates the read and optional writer privilege boundaries
before binding its listener.

The explicit `src/web.rs::API_ROUTE_INVENTORY` is the authoritative advertised
API surface. Required and compatibility routes declare authentication,
pagination, body, and support contracts. Unsupported frontend probes return
stable empty or disabled responses and are not claims of feature support.

Shared HTTP behavior includes Mastodon-compatible CORS and preflight handling,
trailing-slash aliases, cache and `Vary` headers, JSON error envelopes,
request-body limits and deadlines, cursor contracts, and bounded
Rack-compatible query/form/JSON parameter parsing.

### Worker

The worker processes six durable lanes:

- **Ingress** — verified ActivityPub inbox processing;
- **Core** — notification creation, fan-out, and cleanup;
- **Push** — status/account distribution and signed delivery;
- **Pull** — remote objects, threads, profiles, media, and emoji;
- **Mail** — password reset, confirmation, and report mail;
- **Maintenance** — pruning, expiry, purge, and media cleanup.

Startup rejects configured lanes without registered handlers. Execution is at
least once: handlers must make externally visible effects idempotent because a
crash after an effect but before fenced acknowledgement may repeat work.

Poll-expiration readiness establishes the immutable database-clock activation
marker and executes exactly one bounded Maintenance reconciliation segment. If
more candidates remain, that segment commits a durable continuation before its
fenced success watermark and readiness; startup does not synchronously drain an
unbounded continuation chain. Core executors start only after activation exists,
and every expiration handler independently classifies generations at or before
the activation boundary as baseline-only, so a later continuation cannot turn
historical polls into notification or federation effects.

### Preflight

`rustodon preflight` is read-only against PostgreSQL, Redis, and media. It
rejects unsupported configuration, schema drift, unusable signing keys, unsafe
media roots, active unsupported workflows, and non-empty Sidekiq queues. Stable
`PF_*` diagnostics provide remediation hints without exposing secrets or
connection URLs.

### Administration

Rustodon provides a bounded command surface rather than Mastodon's full
administration UI. Commands cover operational migration/readiness, dead jobs,
remote-account refresh, password reset, local-user creation, report/status
moderation, account-stat reconciliation, suspension, and domain policy/purge.
Run `rustodon admin --help` for the authoritative list and arguments.

## Capabilities

### Authentication and OAuth

- Existing bearer tokens retain revocation, expiry, application-owner,
  account-state, and endpoint-scope semantics.
- Browser authentication supports existing passwords, TOTP, backup codes,
  sign-in metadata, Rustodon-managed sessions, logout, and CSRF protection.
- Account recovery supports reset/confirmation mail when SMTP is configured and
  administrator-driven password reset.
- OAuth supports dynamic application registration, authorization-code and
  client-credentials grants, S256 PKCE, consent, revocation, userinfo, and the
  advertised response modes.

Ordinary bearer authentication remains a read path and does not update token
last-used metadata. Browser and OAuth lifecycle writes use the typed writer.

### REST and browser client

The implemented v1 surface includes account/profile reads and updates, search,
status and image-media lifecycle, timelines, lists, filters, markers,
relationships and interactions, conversations, notifications and policies,
reports, application registration, OAuth, web settings, and authenticated user
streaming.

The root web surface serves the pinned production Vite bundle from
`public/packs`, including themes, locales, icons, PWA metadata, and service
worker. It renders escaped initial state plus CSRF/VAPID metadata and guards SPA
deep-link fallback. Bundle provenance and checksums are recorded in
[`public/packs/BUILD.md`](../public/packs/BUILD.md).

### Local media

The media server reuses Mastodon's Paperclip paths for avatars, headers,
attachments, thumbnails, custom emoji, preview cards, provider icons, site
uploads, and processed audio/video. `GET`, `HEAD`, conditionals, and bounded byte
ranges share the same path contract as REST serializers.

Startup securely opens the media root. Requests reject traversal, malformed
styles/metadata, and symlinks without mutating the database or media tree. New
image processing preserves rollback-compatible database metadata and file
layout.

### Federation

Rustodon implements WebFinger, host-meta, NodeInfo, actor and Note routes,
followers/following/outbox collections, signed inboxes, ActivityPub activity
processing, durable delivery, remote object/profile/media fetching, domain
health, and SSRF/TLS/signature protections.

Core activities include Follow/Accept/Reject/Undo, Create/Update/Delete, Like,
Announce, Block, actor updates/deletes, audiences, replies, mentions, media,
custom emoji, and tombstones. Ordering, deduplication, retries, parent/thread
repair, and no-op edit behavior are covered by isolated fixture tests.

## Read and preserve without creating

Rustodon preserves scheduled-status rows when none are pending, lists, pins,
featured/followed tags, filters, preview cards, custom emoji, announcements,
reports, warnings, appeals, severance events, migrations, imports, backups, Web
Push rows, quotes, collections, and unknown future values. Polls and votes have
an implemented REST and ActivityPub lifecycle, including exact-generation expiry
repair. Final-tree restored-fixture, worker, and browser acceptance for that poll
lifecycle remains explicitly deferred; executable test source is not a recorded
pass. Preflight rejects remaining active unsupported workflows instead of
transforming them.

Object storage, Elasticsearch, open registration, LDAP/PAM/CAS/SAML/OIDC and
other SSO providers, scheduled posts, quote creation, relays, advanced
federation extensions, and the complete administration surface are outside the
initial boundary.

## Support and acceptance status

| Area | Implemented evidence | Remaining acceptance |
| --- | --- | --- |
| Startup and preflight | Named schema, startup, configuration, and rejection gates | Production preflight |
| Authentication and OAuth | Unit, schema, differential, and bounded browser fixtures | Broader browser forms and interactive TOTP recording |
| REST and media | Unit, schema, differential, and rollback fixture coverage | Recorded mobile-client flow and production rollback proof |
| Browser client | Pinned bundle, authenticated shell/settings/logout, Home actions, reload persistence | Broader forms and streaming behavior |
| Federation | Differential fixtures and five bounded Mastodon peer scenarios | Final-tree Mastodon reply coverage and broader Mastodon interoperability |
| Durable work | Six lanes, restored-fixture recovery/readiness/shutdown coverage | Sustained production load and hard-power-loss proof |
| Cutover and rollback | Complete disposable-fixture rehearsal | Production maintenance-window rehearsal |

A passing named gate proves only its stated contract. It does not establish
production readiness, untested clients or peers, Pleroma interoperability, or a
later source revision. The [acceptance matrix](v1-acceptance-matrix.md) maps the
full requirement set to implementation and evidence; [Testing](testing.md)
describes the executable gates and their boundaries.
