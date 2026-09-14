# Rustodon

<p align="center">
  <img src="rustodon.png" alt="Rustodon mascot: a cheerful red crab" width="420">
</p>

Rustodon is an experimental Rust implementation intended to become a
mostly-in-place replacement for small Mastodon installations.

The project aims to let an operator stop Mastodon and start Rustodon against
the same PostgreSQL database, local media directory, domain, and persistent
identity. Compatibility is focused on normal user, client, and federation
behavior rather than reproducing Rails implementation details or disposable
cache state.

Public source repository: [github.com/lambadalambda/rustodon](https://github.com/lambadalambda/rustodon).

## Status

Rustodon implements the core authenticated REST, local-media, durable-worker,
streaming, browser-authentication, and ActivityPub paths needed by the Mastodon
4.6.5 compatibility target. The matching Mastodon web bundle is packaged and
served.

Local authorized-NAS evidence covers the required differential lane,
authenticated browser settings and logout, fixture cutover and rollback, and
five guarded scenarios against an actual pinned Mastodon peer. This is bounded
fixture evidence, not a production deployment or certification of every later
tree change.

V1 is not complete. A recorded mobile-client run, production cutover rehearsal,
broader and final-tree federation evidence, Pleroma interoperability, hosted
full-differential/source-contract evidence, and remaining failure/load
hardening are still unclaimed.

The first compatibility target is Mastodon 4.6.5. Supporting one stable schema
first keeps the initial implementation testable; additional Mastodon releases
will be added as explicit compatibility targets.

## V1 Principles

- Preserve existing PostgreSQL data, canonical URLs, IDs, OAuth tokens, and
  ActivityPub signing keys.
- Reuse Mastodon's local Paperclip media layout.
- Support existing users on a closed, small instance before open registration
  and large-instance operations.
- Replace Redis-derived feeds, caches, and queues instead of migrating them.
- Implement the widely interoperable ActivityPub core before newer federation
  extensions.
- Detect unsupported active configuration during preflight instead of silently
  dropping behavior.
- Keep Mastodon-owned tables backward compatible so rollback remains possible.

## Planned V1 Surface

- Existing password, TOTP, OAuth application, and bearer-token authentication.
- Matching Mastodon web assets plus common mobile-client REST APIs.
- Accounts, statuses, replies, images, timelines, follows, favourites,
  bookmarks, boosts, blocks, mutes, conversations, and notifications.
- WebFinger, NodeInfo, ActivityPub discovery, signed inboxes, remote fetching,
  and core social activities.
- PostgreSQL-backed durable work and transactional post-commit events.
- Local filesystem media and a controlled Mastodon-to-Rustodon cutover.

Object storage, Elasticsearch, open registration, SSO providers, polls,
scheduled posts, quote creation, relays, advanced federation extensions, and
the full Mastodon administration surface are intentionally deferred. Existing
quote and collection data remains readable and preserved.

See [the detailed v1 scope](docs/v1-scope.md) for the compatibility boundary,
implementation order, and acceptance criteria. See the [v1 acceptance
matrix](docs/v1-acceptance-matrix.md) for requirement traceability and current
proof status.

## Development

Rustodon uses [Mise](https://mise.jdx.dev/) to pin Rust 1.97.1, Clang 22.1.8,
and cargo-deny 0.20.2. Install Mise 2026.8.4 or newer, then install the project
toolchain:

```console
mise install
```

Run the bounded ordinary development checks with:

```console
mise run check
```

This runs formatting, strict Clippy, default and all-feature debug tests,
all-feature release tests, dependency policy, static fixture checks, vendored
worker-media verification, and offline shell/Python harness regressions. It does
not run ignored database, container, browser, or peer integration tests.

The individual ordinary tasks remain available:

```console
mise run fmt
mise run lint
mise run test
mise run deny
```

Required hosted CI runs these integration lanes separately from `check`:

```console
mise run pinned-source-contracts
mise run mastodon-schema-integration
mise run operational-schema-integration
mise run startup-integration
mise run preflight-integration
mise run worker-media-verify
mise run worker-integration
mise run differential-ci
```

The broader differential, cutover, and browser lanes are scheduled/manual
extended checks:

```console
mise run differential-full
mise run cutover-integration
mise run browser-integration
```

These commands have different source, Podman, browser, and resource
prerequisites; do not replace them with `cargo test -- --ignored` or run fixture
workloads concurrently. See the configured gate map and evidence boundaries in
[`docs/testing-on-nas.md`](docs/testing-on-nas.md), and the hosted job definitions
in [`.github/workflows/ci.yml`](.github/workflows/ci.yml).

### Implemented surface

The `rustodon::mastodon` library keeps Mastodon reads and writes behind separate
typed boundaries. `Repository` remains read-only against Mastodon-owned data:
its SQLx pool is private, connections default to UTC/read-only operation,
ordinary status reads exclude soft-deleted rows, and it exposes no generic ORM,
callback, or save API. `WriteRepository` uses a distinct writer pool; the web
process enables writes only when that pool is configured and otherwise remains
read-only.

The writer covers status and media lifecycle, profiles and web settings,
relationships and interactions, conversations, notifications and policies,
reports, OAuth applications and grants, browser sessions and account recovery,
local-user creation, federation ingestion, and bounded moderation and repair
operations. IDs remain signed `i64` values, while open wrappers preserve unknown
enum strings, integers, and permission bits.

SQLx is built only for PostgreSQL with Tokio, Chrono, JSON, and `inet` support;
compile-time query macros and unrelated database drivers are disabled. Saphyr
parses Rails YAML safely while the library retains original YAML and JSON text
byte-for-byte. Token and private-key values use redacted opaque wrappers.

Existing OAuth bearer tokens are authenticated with Mastodon-compatible
revocation, expiration, application-owner, account-state, and endpoint-scope
semantics. Ordinary bearer authentication remains a read path and does not
update token last-used metadata. Browser authentication is a separate writable
path: it supports existing passwords, TOTP and backup codes, records sign-in
metadata, creates Rustodon-managed browser sessions, and exposes password
recovery and essential security/settings flows.

The production web process serves an explicit, tested REST method/path inventory
rather than a generic fallback API. The implemented v1 surface includes account
and search reads, status/media creation and lifecycle, timelines, relationships
and interactions, conversations, notifications and policies, reports, OAuth
authorization and revocation, browser authentication and settings,
authenticated user streaming, local Paperclip media, and core ActivityPub
discovery, inbox, outbox, actor, Note, and collection routes. Unsupported
frontend probes are explicitly declared and return stable disabled or empty
responses; they are not claims of feature support.

Rails-versus-Rust differential tests verify synchronous, database-free REST
serializers and HTTP behavior against Mastodon 4.6.5. REST protocol handling
centralizes CORS and preflight behavior, trailing slashes, cache and `Vary`
headers, JSON error envelopes, a 4 MiB public-request cap, authenticated 99 MiB
body limits, a bounded 30-second body-read deadline, endpoint cursor contracts,
and Rack-compatible query/form/JSON parameter parsing. The authoritative route
inventory is `src/web.rs::API_ROUTE_INVENTORY`; the detailed support and
acceptance boundary is in
[`docs/v1-acceptance-matrix.md`](docs/v1-acceptance-matrix.md).

The root web surface serves the pinned production Vite bundle from
`public/packs`, including hashed chunks, themes, locales, icons, the PWA
manifest, favicon, and service worker. It renders the Mastodon shell with
escaped initial state, CSRF/VAPID metadata, and guarded SPA deep-link fallback.
The bundle provenance and checksums are recorded in
[`public/packs/BUILD.md`](public/packs/BUILD.md).

The web process also serves database-referenced local Paperclip media from the
existing filesystem tree. Account avatars and headers, media files and
thumbnails, custom emoji, preview cards and provider icons, site uploads, and
existing processed audio/video use one shared path contract with REST
serializers. `GET`, `HEAD`, conditional requests, and bounded streaming byte
ranges match the pinned Rails/Rack behavior. Startup retains a securely opened
media-root descriptor; requests reject traversal, invalid styles and metadata,
and symlinks without mutating the database or media tree.

### Mastodon compatibility fixture

The first compatibility baseline is pinned to Mastodon v4.6.5 commit
`1440d55b139e39ec722c2a3db7f60b66cd889048`, schema version
`20260611150940`. Its release-versioned database, catalog, migration, and local
media artifacts live under [`fixtures/mastodon/v4.6.5/`](fixtures/mastodon/v4.6.5/).

Obtain the matching source without vendoring it, generate the fixture, and run
the fast checksum/metadata verification with:

```console
mise run fixture-obtain
mise run fixture-generate
mise run fixture-verify
```

The source checkout is stored under ignored `target/`. Generation rejects a
dirty source tree, reads migration/media inputs from pinned Git blobs, and uses
the pinned `linux/amd64` child manifests for Mastodon and PostgreSQL 14. It uses
dedicated `.invalid` domains and the dedicated
`rustodon_mastodon_v4_6_5_fixture` database, never project or Mastodon `.env`
files. Redis is intentionally not started because direct Paperclip processing
and the verified Rails schema/model/serializer paths do not require it.

The following container-backed fixture commands are separate from
`mise run check`. Some are required hosted-CI lanes, some are scheduled/manual
lanes, and the restore/reproduction commands are fixture-maintenance tools; use
the gate map above for their exact classification and prerequisites.

```console
mise run fixture-restore-verify
mise run fixture-repro
mise run mastodon-schema-integration
mise run operational-schema-integration
mise run startup-integration
mise run worker-integration
mise run preflight-integration
mise run differential
mise run cutover-integration
mise run browser-integration
```

`fixture-restore-verify` restores the checked dump and verifies it through SQL
and Mastodon Rails, including all 17 known notification types, a filtered unknown
type, and Paperclip media. `fixture-repro` regenerates every artifact and performs
a recursive byte-for-byte comparison. `mastodon-schema-integration` publishes
PostgreSQL on a random loopback port, creates a LOGIN role limited to database
`CONNECT`, schema `USAGE`, and table `SELECT`, and runs the ignored Rust
integration tests. Those tests also prove DML, `TRUNCATE`, and schema creation
fail after trying to disable the role's default read-only setting.
The operational-schema task invokes the real admin command through a dedicated
non-superuser migrator, exercises repeat and concurrent creation from absent and
empty schemas, rejects catalog and ownership drift, and proves Mastodon's
`public` catalog, data, ownership, and privileges remain unchanged before Rails
re-verifies the fixture.
The worker task uses a separate `NOINHERIT` runtime login, proves its exact
operational grants and inability to mutate Mastodon objects, and exercises
transactional enqueue/outbox dispatch, crash recovery, leases, cancellation,
retries, dead letters, resource limits, readiness, and bounded shutdown.
These Podman tasks currently require GNU/Linux x86-64; labeled PostgreSQL
volumes are removed and checked after each task, and bind mounts support SELinux
relabeling.

The differential task starts pinned Mastodon 4.6.5 and a Rust fixture response
against independent database and media clones, sends each case's exact HTTP
request to both, and compares status, declared headers, and canonical JSON. It
also checks logical database rows and media contents and metadata before and
after the request. Its media case exercises all nine checked fixture files,
`GET`, `HEAD`, single and multipart ranges, conditionals, and hardened rejection
paths.
Run one case by its Rust test name without executing the complete suite:

```console
mise run differential -- instance_v2
mise run differential -- oauth_bearer_authentication
mise run differential -- status_authorization_matrix
mise run differential -- core_rest_serializers
mise run differential -- rest_protocol_contracts
mise run differential -- federation_discovery
mise run differential -- local_web_client_shell
```

Mismatch output identifies the status, header, JSON path, table/key, or media
path that differs. Narrow normalization rules are declared centrally and
validate request IDs, generated RFC 3339 timestamps, or prefixed random test
tokens before replacing them. The harness compares observable contracts, not
Rails callbacks, SQL ordering, Redis keys, or Sidekiq payload representation.
Its databases, media roots, Redis, and HTTP ports are run-marked test resources
under `target/`; production-looking URLs and unmarked paths are rejected.

### Browser and peer evidence

The opt-in browser lane starts Rustodon inside the HTTPS cutover fixture and
drives the pinned frontend in Chromium through `agent-browser`. It checks the
anonymous and authenticated shells, React mounting and SPA navigation, settings
and logout, real Home boost/reply controls, leading and trailing web-settings
PUTs, persistence after reload, unexpected API failures, and the enclosing
fixture cutover/rollback comparison. Set `RUSTODON_BROWSER_RECORD` to a WebM
path and `RUSTODON_BROWSER_SCREENSHOT` to a PNG path when retaining operator
evidence. This is not coverage of every browser form, WebSocket, or EventSource
flow.

Five guarded peer scenarios—public delivery, private/direct visibility, Note
create/update/delete, profile updates, and interactions/Undo—passed in both
directions against an actual pinned Mastodon 4.6.5 process on the authorized
NAS. Those runs preceded later browser/API changes and therefore are not a fresh
certification of the exact final tree. They also do not establish Pleroma
compatibility, universal federation interoperability, production behavior, or
concurrent-load safety.

The peer commands are manual and restricted to their authorized host and
workspace:

```console
mise run peer-public
mise run peer-privacy
mise run peer-notes
mise run peer-profile
mise run peer-interactions
```

See [`docs/federation-peer-smoke.md`](docs/federation-peer-smoke.md) and the
evidence boundaries in [`docs/testing-on-nas.md`](docs/testing-on-nas.md).

Before a cutover, run `rustodon preflight` with the Mastodon production
environment. It exits nonzero for unsupported configuration, schema drift,
unusable signing keys, unsafe media roots, active unsupported workflows, or
non-empty Sidekiq work, and prints stable `PF_*` diagnostic codes with
remediation hints. It is read-only against PostgreSQL and Redis and performs no
media writes. The minimal environment surface is:

- `LOCAL_DOMAIN`, optional `WEB_DOMAIN` and `ALTERNATE_DOMAINS`
- optional `LIMITED_FEDERATION_MODE=true` (or legacy `WHITELIST_MODE=true`) to
  use the Mastodon domain allow-list
- `PRIMARY_DATABASE_URL`, `DATABASE_URL`, or Mastodon's `DB_*` variables
- `WRITE_DATABASE_URL` for normal writable v1 operation, using a dedicated
  least-privilege writer role against the same PostgreSQL database; omit it only
  for read-only inspection or the initial preflight, in which case the web
  process does not mutate Mastodon-owned data and worker lane defaults are
  restricted
- absolute `PAPERCLIP_ROOT_PATH` and optional `PAPERCLIP_ROOT_URL`
- optional explicit `TRUSTED_PROXY_IP` CIDRs and SMTP variables; forwarded
  metadata is ignored when no trusted proxies are configured
- optional `USER_ACTIVE_DAYS` to match Mastodon's configurable active-follower
  notification window (default `7`)
- ActivityPub inbox requests are bounded to `300` per trusted client IP in five
  minutes before body buffering or signature-key work
- `SECRET_KEY_BASE` and the three `ACTIVE_RECORD_ENCRYPTION_*` secrets
- optional `SIDEKIQ_REDIS_*` or `REDIS_*` settings for the queue-drain check

Object storage, read replicas, LDAP/PAM/CAS/SAML/OIDC, and SSO-only login are
reported as fatal v1 incompatibilities instead of being partially emulated.
Secret values and connection URLs are redacted from configuration and preflight
diagnostics.

The steps below describe Rustodon's isolated operational schema, not the entire
writable cutover. A production cutover also requires the guarded
`refresh_instances` function, exact writer grants, distinct migration and
runtime roles, `WRITE_DATABASE_URL`, and a second successful preflight with the
complete environment. Follow [`docs/cutover.md`](docs/cutover.md) from start to
finish; do not treat the migration command alone as deployment preparation.

Create or upgrade Rustodon's separately versioned operational schema after the
initial read-only preflight and while Mastodon application processes are
stopped:

```console
rustodon admin migrate-operational-schema
```

The command is transactional and idempotent. It serializes with Rustodon and
Active Record migrations, validates the pinned Mastodon schema before and after
DDL, and creates only `rustodon.schema_migrations` plus the eight operational
tables for durable jobs, outbox events, idempotency keys, ordering markers,
domain health, process heartbeats, shared rate-limit windows, and remote-fetch
leases. It never performs automatic web-startup DDL or changes objects under
`public`. The migration role needs database
`CONNECT` and `CREATE`, `USAGE` on `public`, and `SELECT` on its tables and
sequences; it does not need superuser, role-management, or Mastodon write
privileges.

Run workers through a dedicated `NOINHERIT` login, not the schema owner. After
migration, an administrator must revoke inherited database/schema creation
rights and grant the runtime role only Mastodon reads plus Rustodon's operational
DML and identity-sequence use. Worker startup validates that exact boundary,
including direct and `PUBLIC` grants, and refuses privileged or drifted roles.
The production process registers ingress, maintenance, core, push, pull, and mail
handlers when their corresponding dependencies are configured. `WORKER_LANES`
defaults to the configured supported set; configuring a lane without a
registered handler fails startup rather than publishing false readiness.

### Administrative commands

Rustodon exposes a bounded administrative surface rather than Mastodon's full
administration UI. Operational commands migrate the isolated schema, inspect
worker readiness, and list dead jobs:

```console
rustodon admin migrate-operational-schema
rustodon admin worker-readiness
rustodon admin dead-jobs --limit 50
```

Account and recovery commands refresh an existing remote account, reset a
password, create an explicit local user, and reconcile account statistics.
Moderation commands resolve or reopen reports, delete statuses with
authorization, suspend or unsuspend accounts, and manage or purge domain
policies. Their presence does not imply support for Mastodon's full
administration surface.

Use `rustodon admin --help` and command-specific `--help` output for current
arguments and safety requirements. The complete command inventory is also
listed in the [v1 acceptance matrix](docs/v1-acceptance-matrix.md).

Worker execution is at least once. Handlers must make externally visible effects
idempotent because a crash after an effect but before fenced acknowledgement can
repeat the job.
See the [fixture documentation](fixtures/mastodon/v4.6.5/README.md) for test
identities, key/media provenance, normalization, and the later-release update
process.

The operational cutover sequence, smoke checks, rollback triggers, and
Mastodon restart procedure are documented in
[`docs/cutover.md`](docs/cutover.md).

Inspect the planned process modes with:

```console
mise exec -- cargo run -- --help
mise exec -- cargo run -- web --help
mise exec -- cargo run -- worker --help
mise exec -- cargo run -- admin --help
```

The first compatibility milestones pin Mastodon 4.6.5 fixtures, map its schema,
and build a differential test harness before any production data is written.
The initial development and CI target is `x86_64-unknown-linux-gnu`; other
platform targets will be added when they have automated coverage.

Open work is tracked in [meta/issues.md](meta/issues.md). Each entry links to a
detailed issue under `meta/issues/`.

Important findings and decisions are recorded in [DEVLOG.md](DEVLOG.md).

## Repository Layout

```text
.cargo/        Target-specific Cargo configuration
.github/       Continuous integration workflows
src/           Rust application source
tests/         Integration tests
docs/          Project scope and design documentation
fixtures/      Release-versioned compatibility databases and local media
meta/          Repository-local issue tracker
meta/issues/   Detailed issue specifications
tools/         Reproducible fixture and development tooling
```

## Compatibility Philosophy

Rustodon should match persisted data contracts, authorization rules, REST
entities, and federation messages that real clients and peers depend on. It
does not need to match callback structure, SQL ordering, Redis key layout,
Sidekiq payloads, or obscure Rails error wording.

Read behavior will be compared against Mastodon using shared fixtures. Write
behavior will be compared using cloned databases, media trees, emitted
ActivityPub messages, and durable jobs.

## License

No license has been selected yet.
