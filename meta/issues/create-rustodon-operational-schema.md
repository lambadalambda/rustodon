# Create the Rustodon operational schema

## Summary

Add a separately versioned `rustodon` PostgreSQL schema for operational state.

## Requirements

- Store durable jobs, outbox events, idempotency keys, ordering markers, domain
  health, worker/scheduler heartbeats, and shared rate-limit windows without
  changing Mastodon objects.
- Make creation and upgrades idempotent and reject unknown schema versions.

## Acceptance Criteria

- Catalog diffs prove Mastodon-owned tables, functions, sequences, constraints,
  and indexes are unchanged and rollback remains possible.

## Notes

- Creation and upgrades are explicit through
  `rustodon admin migrate-operational-schema`; web startup performs no DDL.
- Version 1 contains the original six operational tables plus the separately
   versioned `rustodon.schema_migrations` ledger. Version 2 adds the shared
   `rate_limit_windows` table, and Version 3 adds expiring remote-fetch leases,
   without changing Mastodon-owned objects.
- Migrations use a non-superuser role, run transactionally under Rustodon and
  Active Record migration locks, and reject unknown versions, checksums,
  ownership, ACL, object, dependency, or supported-Mastodon drift.
- The PostgreSQL 14 integration gate proves fresh, repeated, and concurrent
  migration, unsupported-database rollback, unchanged `public` catalog/schema/
  data/access snapshots, and final Mastodon Rails readability.
