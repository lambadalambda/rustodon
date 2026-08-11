# Rustodon

Rustodon is an experimental Rust implementation intended to become a
mostly-in-place replacement for small Mastodon installations.

The project aims to let an operator stop Mastodon and start Rustodon against
the same PostgreSQL database, local media directory, domain, and persistent
identity. Compatibility is focused on normal user, client, and federation
behavior rather than reproducing Rails implementation details or disposable
cache state.

## Status

Rustodon is in its initial compatibility-harness phase. A read-only Mastodon
4.6.5 schema library and its fixture-backed integration workflow are available,
but Rustodon is not yet usable as a Mastodon server.

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
These Podman tasks currently require GNU/Linux x86-64; labeled PostgreSQL
volumes are removed and checked after each task, and bind mounts support SELinux
relabeling.

The differential task starts pinned Mastodon 4.6.5 and a Rust fixture response
against independent database and media clones, sends each case's exact HTTP
request to both, and compares status, declared headers, and canonical JSON. It
also checks logical database rows and media hashes before and after the request.
Run one case by its Rust test name without executing the complete suite:

```console
mise run differential -- instance_v2
```

Mismatch output identifies the status, header, JSON path, table/key, or media
path that differs. Narrow normalization rules are declared centrally and
validate request IDs, generated RFC 3339 timestamps, or prefixed random test
tokens before replacing them. The harness compares observable contracts, not
Rails callbacks, SQL ordering, Redis keys, or Sidekiq payload representation.
Its databases, media roots, Redis, and HTTP ports are run-marked test resources
under `target/`; production-looking URLs and unmarked paths are rejected.
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
