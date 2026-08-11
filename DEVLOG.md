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
