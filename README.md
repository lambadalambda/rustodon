# Rustodon

Rustodon is an experimental Rust implementation intended to become a
mostly-in-place replacement for small Mastodon installations.

The project aims to let an operator stop Mastodon and start Rustodon against
the same PostgreSQL database, local media directory, domain, and persistent
identity. Compatibility is focused on normal user, client, and federation
behavior rather than reproducing Rails implementation details or disposable
cache state.

## Status

Rustodon is in its initial read-only compatibility phase. A production Axum web
process now serves Mastodon 4.6.5 account, status, relationship, timeline,
collection, and instance reads directly from PostgreSQL, but writes,
federation, workers, and operations are not complete enough for a cutover.

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
implementation order, and acceptance criteria.

## Development

Rustodon uses [Mise](https://mise.jdx.dev/) to pin Rust 1.97.1, Clang 22.1.8,
and cargo-deny 0.20.2. Install Mise 2026.8.4 or newer, then install the project
toolchain:

```console
mise install
```

Run every local and CI quality gate with:

```console
mise run check
```

The individual tasks are also available:

```console
mise run fmt
mise run lint
mise run test
mise run deny
```

The `rustodon::mastodon` library exposes focused PostgreSQL reads for the v1
account, status, relationship, notification, policy, setting, quote,
collection, poll, and signing-key data. It is intentionally read-only: its
SQLx pool is private, connections default to UTC/read-only operation, normal
status reads exclude soft-deleted rows, and there are no save, update, callback,
or generic ORM APIs. IDs remain signed `i64` values and open wrappers retain
unknown enum strings/integers and permission bits.

SQLx is built only for PostgreSQL with Tokio, Chrono, JSON, and `inet` support;
compile-time query macros and unrelated database drivers are disabled. Saphyr
parses Rails YAML safely while the library retains the original YAML and JSON
text byte-for-byte. Token and private-key values use redacted opaque wrappers.

The library also authenticates existing OAuth bearer tokens through one
secret-minimizing joined read. It preserves Mastodon's exact revocation,
expiration, application-only, owner-state, and endpoint-specific scope
semantics while returning Mastodon-compatible HTTP authentication failures.
This read-only milestone intentionally does not update token last-used or user
sign-in metadata; those writes are deferred to the authenticated-write phase.

The library also exposes synchronous, database-free Mastodon 4.6.5 REST
serializers fed by batched read-only projections. Current coverage includes
instance v1/v2, accounts, credentials, relationships, statuses and nested
entities, collections, filters, markers, and every known notification type.
Rails-versus-Rust differential tests verify exact JSON behavior while keeping
root status authorization in the dedicated read-endpoint milestone.

The production web router currently serves instance v1/v2, instance rules,
disabled translation languages, account show/lookup/search/credentials/
relationships, account statuses/followers/following, markers, status
show/source/history/context, reverse favourite/boost actor reads,
account filters/lists/featured tags, public account featured tags,
PostgreSQL-backed home/public/tag/list timelines, and
favourites/bookmarks/blocks/mutes. Root status authorization is kept separate
from account-status and context filtering;
public, unlisted, private, direct, and limited visibility is checked against
current follows, active or silent mentions, blocks, domain blocks, suspended
authors, and soft deletion. Pagination and authorization are differentially
checked against Mastodon 4.6.5 through dedicated fixture cases.

The router also centralizes Mastodon-compatible REST protocol behavior for its
 explicit 32-route inventory: CORS and preflight handling, trailing slashes,
cache and `Vary` headers, JSON error envelopes, the 99 MiB body limit, and
endpoint cursor contracts. Query, form, and registered JSON request bodies use
bounded Rack-compatible scalar/array/hash parsing, including Rails parameter
limits and malformed-shape behavior. Unsupported routes are never advertised
by preflight responses.

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

The following checks are intentionally separate from normal CI because they
start Podman containers:

```console
mise run fixture-restore-verify
mise run fixture-repro
mise run mastodon-schema-integration
mise run operational-schema-integration
mise run worker-integration
mise run preflight-integration
mise run differential
```

The first restores the checked dump and verifies it through SQL and Mastodon
Rails, including all 17 known notification types, a filtered unknown type, and
Paperclip media. The second regenerates every artifact and performs a recursive
byte-for-byte comparison. The third publishes PostgreSQL on a random loopback
port, creates a LOGIN role limited to database `CONNECT`, schema `USAGE`, and
table `SELECT`, and runs the ignored Rust integration tests. Those tests also
prove DML, `TRUNCATE`, and schema creation fail after trying to disable the
role's default read-only setting.
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
```

Mismatch output identifies the status, header, JSON path, table/key, or media
path that differs. Narrow normalization rules are declared centrally and
validate request IDs, generated RFC 3339 timestamps, or prefixed random test
tokens before replacing them. The harness compares observable contracts, not
Rails callbacks, SQL ordering, Redis keys, or Sidekiq payload representation.
Its databases, media roots, Redis, and HTTP ports are run-marked test resources
under `target/`; production-looking URLs and unmarked paths are rejected.

Before a cutover, run `rustodon preflight` with the Mastodon production
environment. It exits nonzero for unsupported configuration, schema drift,
unusable signing keys, unsafe media roots, active unsupported workflows, or
non-empty Sidekiq work, and prints stable `PF_*` diagnostic codes with
remediation hints. It is read-only against PostgreSQL and Redis and performs no
media writes. The minimal environment surface is:

- `LOCAL_DOMAIN`, optional `WEB_DOMAIN` and `ALTERNATE_DOMAINS`
- `PRIMARY_DATABASE_URL`, `DATABASE_URL`, or Mastodon's `DB_*` variables
- absolute `PAPERCLIP_ROOT_PATH` and optional `PAPERCLIP_ROOT_URL`
- optional explicit `TRUSTED_PROXY_IP` CIDRs and SMTP variables; forwarded
  metadata is ignored when no trusted proxies are configured
- `SECRET_KEY_BASE` and the three `ACTIVE_RECORD_ENCRYPTION_*` secrets
- optional `SIDEKIQ_REDIS_*` or `REDIS_*` settings for the queue-drain check

Object storage, read replicas, LDAP/PAM/CAS/SAML/OIDC, and SSO-only login are
reported as fatal v1 incompatibilities instead of being partially emulated.
Secret values and connection URLs are redacted from configuration and preflight
diagnostics.

Create or upgrade Rustodon's separately versioned operational schema explicitly
after preflight and while Mastodon application processes are stopped:

```console
rustodon admin migrate-operational-schema
```

The command is transactional and idempotent. It serializes with Rustodon and
Active Record migrations, validates the pinned Mastodon schema before and after
DDL, and creates only `rustodon.schema_migrations` plus the six operational
tables for durable jobs, outbox events, idempotency keys, ordering markers,
domain health, and process heartbeats. It never performs automatic web-startup
DDL or changes objects under `public`. The migration role needs database
`CONNECT` and `CREATE`, `USAGE` on `public`, and `SELECT` on its tables and
sequences; it does not need superuser, role-management, or Mastodon write
privileges.

Run workers through a dedicated `NOINHERIT` login, not the schema owner. After
migration, an administrator must revoke inherited database/schema creation
rights and grant the runtime role only Mastodon reads plus Rustodon's operational
DML and identity-sequence use. Worker startup validates that exact boundary,
including direct and `PUBLIC` grants, and refuses privileged or drifted roles.
The built-in process currently registers only the maintenance handler, so
`WORKER_LANES` defaults to `maintenance`; configuring a lane without a registered
handler fails startup rather than publishing false readiness. Later feature
milestones register ingress, core, push, pull, and mail handlers with their
corresponding lanes.

Operational inspection is available through:

```console
rustodon admin worker-readiness
rustodon admin dead-jobs --limit 50
```

Worker execution is at least once. Handlers must make externally visible effects
idempotent because a crash after an effect but before fenced acknowledgement can
repeat the job.
See the [fixture documentation](fixtures/mastodon/v4.6.5/README.md) for test
identities, key/media provenance, normalization, and the later-release update
process.

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
