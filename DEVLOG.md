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
