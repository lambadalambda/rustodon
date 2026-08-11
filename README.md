# Rustodon

Rustodon is an experimental Rust implementation intended to become a
mostly-in-place replacement for small Mastodon installations.

The project aims to let an operator stop Mastodon and start Rustodon against
the same PostgreSQL database, local media directory, domain, and persistent
identity. Compatibility is focused on normal user, client, and federation
behavior rather than reproducing Rails implementation details or disposable
cache state.

## Status

Rustodon is in its initial planning and compatibility-harness phase. It is not
yet usable as a Mastodon server.

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

Implementation has not started. The first milestones establish the Rust
workspace, pin Mastodon 4.6.5 fixtures, map its schema, and build a differential
test harness before any production data is written.

Open work is tracked in [meta/issues.md](meta/issues.md). Each entry links to a
detailed issue under `meta/issues/`.

Important findings and decisions are recorded in [DEVLOG.md](DEVLOG.md).

## Repository Layout

```text
docs/          Project scope and design documentation
meta/          Repository-local issue tracker
meta/issues/   Detailed issue specifications
```

Rust source and Cargo workspace layout will be introduced by the bootstrap
issue rather than committed speculatively.

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
