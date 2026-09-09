# Validate Mastodon writer privileges at startup

## Summary

Fail closed when the optional `WRITE_DATABASE_URL` points at an unsafe or
unusable PostgreSQL role.

## Requirements

- Validate the configured writer connection before web or worker readiness.
- Reject superusers, role/database owners, role membership, role creation,
  database creation, replication, bypass-RLS, schema creation, and writable
  default privileges.
- Keep diagnostics secret-free and preserve the read-only default when no
  writer is configured.

## Acceptance Criteria

- A valid least-privilege writer is accepted.
- A writer with each unsafe role or privilege attribute is rejected before the
  process binds or claims work.
- Writer validation does not mutate the Mastodon database or require a
  generic SQL write surface.

## Progress

- Added a bounded, read-only writer catalog check to startup validation.
- Validated role login state, expiry, ownership, membership, database/schema
  privileges, default privileges, required current write/read capabilities,
  and object ownership.
- Operational-schema ACL comparison now excludes only the explicitly
  configured writer role in addition to the runtime role.
- Guarded startup integration covers a valid writer and web/worker refusal for
  unsafe role attributes, database/schema creation, membership, default
  privileges, and database ownership.
- The writer capability query now also requires the column-level account and
  user privileges, the current status/relationship/notification/media/OAuth
  table capabilities, required sequences, and `timestamp_id(text)` execution;
  the fixture differential writer grants match that SQL contract. The optional
  CLI preflight now runs the writer check as well as startup validation.
- Refresh-function validation now requires the function to be owned by the
  `public.instances` owner, use `SECURITY DEFINER`, and declare the exact
  `search_path = pg_catalog, public` configuration. Production provisioning is
  documented in `docs/mastodon-refresh-instances.sql` and `docs/cutover.md`,
  and guarded startup mutations reject each unsafe function state.
- The writer contract now requires the `quotes` read, remote tombstone
  read/insert and sequence capabilities, and rejects direct table-level `INSERT`
  grants outside the supported allowlist. Guarded startup coverage exercises
  both missing tombstone access and an unexpected quote insert grant.
- PostgreSQL 14 ACL validation now preserves built-in default public privileges
  while rejecting public drift on non-table objects, writable default schema
  privileges including `CREATE`, large-object compatibility settings and ACLs,
  and ownership across the remaining catalog object classes readable by the
  least-privilege writer role.
- Guarded startup coverage now exercises public foreign-object and tablespace
  grants plus default schema privileges. The restored Mastodon 4.6.5 startup
  safety integration passes 3/3 tests, and the standard repository check passes.
- The writer contract now requires `UPDATE` on `account_deletion_requests` for
  the purge path's `FOR UPDATE` lock. The restored fixture and guarded startup
  coverage verify that the least-privilege writer can acquire that lock.
