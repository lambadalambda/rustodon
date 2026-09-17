# Rustodon

<p align="center">
  <img src="rustodon.png" alt="Rustodon mascot: a cheerful red crab" width="420">
</p>

Rustodon is an experimental Rust implementation intended to become a
mostly-in-place replacement for small Mastodon installations. It is designed to
reuse the same PostgreSQL database, local Paperclip media, domain, OAuth
identities, and ActivityPub signing keys while replacing Rails application
processes and Redis-derived state.

Public source: [github.com/lambadalambda/rustodon](https://github.com/lambadalambda/rustodon).

## Status

The first compatibility target is Mastodon 4.6.5. Rustodon implements the core
authenticated REST, browser authentication and settings, local media, streaming,
durable work, moderation, and ActivityPub paths required by that target. The
matching Mastodon web bundle is packaged and served.

Automated and isolated-fixture evidence covers ordinary quality gates, schema
and startup safety, the required Rails-versus-Rust differential lane, browser
settings and logout, cutover/rollback, durable workers, and five bounded
Mastodon peer scenarios. This is not production certification.

V1 remains incomplete. Outstanding acceptance includes a recorded mobile-client
run, broader browser and interactive-TOTP coverage, a production cutover and
rollback rehearsal, and final-tree Mastodon peer evidence including replies.
Pleroma interoperability and broader peer coverage remain separate future work.

## Capabilities and limits

Rustodon aims to preserve persisted data and externally observable behavior, not
Rails callbacks, SQL ordering, Redis keys, Sidekiq payloads, or disposable cache
state.

- Existing passwords, TOTP and backup codes, OAuth applications/tokens, and
  Rustodon-managed browser sessions.
- Accounts, profiles, statuses, replies, image media, timelines, relationships,
  interactions, conversations, notifications, reports, and essential settings.
- WebFinger, NodeInfo, ActivityPub discovery, signed inboxes, durable delivery,
  remote fetching, and core social activities.
- PostgreSQL-backed jobs, outbox events, ordering, shared rate limits, and remote
  fetch coordination.
- Existing local Paperclip media layout and a controlled Mastodon-to-Rustodon
  cutover with rollback compatibility.

Object storage, Elasticsearch, open registration, SSO providers, scheduled posts,
quote creation, relays, advanced federation extensions, and the full Mastodon
administration surface are deferred. Existing unsupported data is preserved or
rejected by preflight rather than silently discarded.

## Documentation

- [Architecture and capabilities](docs/architecture.md)
- [Testing and compatibility fixtures](docs/testing.md)
- [Cutover and rollback](docs/cutover.md)
- [Detailed v1 scope](docs/v1-scope.md)
- [V1 acceptance matrix](docs/v1-acceptance-matrix.md)
- [Federation peer smoke tests](docs/federation-peer-smoke.md)
- [Mastodon 4.6.5 fixture](fixtures/mastodon/v4.6.5/README.md)
- [Frontend bundle provenance](public/packs/BUILD.md)

## Development

Rustodon uses [Mise](https://mise.jdx.dev/) to pin Rust 1.97.1, Clang 22.1.8,
and cargo-deny 0.20.2. Install Mise 2026.8.4 or newer, then run:

```console
mise install
mise run check
```

`check` runs formatting, strict lint, ordinary default/all-feature debug and
release tests, dependency policy, static fixture checks, vendored worker-media
verification, and offline harness tests. Container-backed, browser, differential,
and peer integrations are separate named gates; see [Testing](docs/testing.md).

Inspect the process and command surface with:

```console
mise exec -- cargo run -- --help
mise exec -- cargo run -- web --help
mise exec -- cargo run -- worker --help
mise exec -- cargo run -- admin --help
```

## Operations

The supported deployment model is a small Mastodon 4.6.5 installation using
PostgreSQL and local media, with no concurrent Mastodon and Rustodon writers.
Never treat schema migration alone as a cutover.

```console
rustodon preflight
rustodon admin migrate-operational-schema
rustodon admin worker-readiness
```

A real cutover requires snapshots, drained queues, separate least-privilege
roles, a second complete preflight, smoke testing, monitoring, and a tested
rollback path. Follow the [cutover runbook](docs/cutover.md) from start to finish.

## Repository layout

```text
src/       Rust application source
tests/     Integration tests
docs/      Architecture, testing, and operations
fixtures/  Versioned compatibility databases and media
tools/     Reproducible fixture and development tooling
meta/      Project issues and decision records
```

Open work is tracked in [meta/issues.md](meta/issues.md). Important findings and
decisions are recorded in [DEVLOG.md](DEVLOG.md).

## License

No license has been selected yet.
