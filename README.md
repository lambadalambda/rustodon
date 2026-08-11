# Rustodon

Rustodon is an experimental Rust implementation intended to become a
mostly-in-place replacement for small Mastodon installations.

The project aims to let an operator stop Mastodon and start Rustodon against
the same PostgreSQL database, local media directory, domain, and persistent
identity. Compatibility is focused on normal user, client, and federation
behavior rather than reproducing Rails implementation details or disposable
cache state.

## Status

Rustodon is in its initial compatibility-harness phase. The Rust workspace and
quality gates are established, but it is not yet usable as a Mastodon server.

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
meta/          Repository-local issue tracker
meta/issues/   Detailed issue specifications
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
