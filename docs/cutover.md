# Rustodon Cutover and Rollback

This runbook applies to the supported v1 deployment: one small Mastodon 4.6.5
instance, PostgreSQL, local Paperclip media, closed registration, and no
concurrent Mastodon/Rustodon writers.

## Before The Window

1. Confirm the Rustodon binary, Mise toolchain, configuration, and local media
   root are the intended versions. Keep `LOCAL_DOMAIN`, `WEB_DOMAIN`,
   `ALTERNATE_DOMAINS`, PostgreSQL URLs, Paperclip paths, and encryption/signing
   secrets unchanged.
2. Take a PostgreSQL snapshot and a filesystem snapshot or verified copy of the
   complete Paperclip root. Record the snapshot IDs and restore commands.
3. Resolve unsupported configuration reported by preflight. In particular,
   scheduled statuses, pending account deletions, WebAuthn-only users, active
   relays, object storage, SSO providers, and non-empty Sidekiq queues must be
   handled before cutover.
4. Stop new writes at the proxy, allow existing requests to finish, and drain
   Sidekiq. Confirm no Mastodon web, streaming, or worker process is still
   writing the database.

Run the initial read-only checks while Mastodon is still stopped, without a
writer URL because the Rustodon operational schema and refresh function are
provisioned below:

```console
env -u WRITE_DATABASE_URL rustodon preflight
```

The command must finish successfully. Preserve its output with the deployment
record; `PF_*` codes are intended for remediation and rollback diagnostics.

## Migration And Startup

1. Start PostgreSQL with the snapshot selected for the window.
2. Run the operational-schema migration with the dedicated migrator role:

   ```console
   rustodon admin migrate-operational-schema
   ```

   This creates only Rustodon's `rustodon` schema. It does not migrate or
   alter Mastodon-owned tables.
3. As the owner of Mastodon's `public.instances` materialized view, provision
   `docs/mastodon-refresh-instances.sql`:

   ```console
   psql "$MASTODON_OWNER_DATABASE_URL" -v ON_ERROR_STOP=1 \
     -f docs/mastodon-refresh-instances.sql
   ```

    Do not grant the writer `REFRESH` directly. `rustodon preflight` verifies
   that the function is owned by the materialized-view owner, is
   `SECURITY DEFINER`, and has the fixed `pg_catalog, public` search path.
4. Create or select a dedicated writer role with no memberships, ownership,
   superuser, database/schema creation, replication, or bypass-RLS privileges.
    Set its password through the secret manager or `psql`'s `\password` command.
   After the operational migration and refresh function have been provisioned,
   apply the complete writer contract as a database administrator or owner with
   grant rights on both schemas:

   ```console
   psql "$POSTGRES_ADMIN_DATABASE_URL" -v ON_ERROR_STOP=1 \
     --set writer_role="$RUSTODON_WRITER_ROLE" \
     -f docs/mastodon-writer-grants.sql
   ```

   The script removes default `PUBLIC` database/schema/object grants and stale
   writer grants, then applies the exact Mastodon and Rustodon table, column,
   sequence, and function capabilities checked by preflight. The writer role
   must be distinct from the runtime role.
5. Grant the runtime role only its documented read access to Mastodon tables and
   its operational-schema access. Configure `WRITE_DATABASE_URL` to the same
   PostgreSQL database as `DATABASE_URL`, using only the separately validated
   typed writer role.

   Run the following as the owner of the Rustodon operational schema (the role
   that ran `migrate-operational-schema`), replacing the two role variables with
   the production role names:

   ```console
   psql "$RUSTODON_OPERATIONAL_OWNER_DATABASE_URL" -v ON_ERROR_STOP=1 \
     --set runtime_role="$RUSTODON_RUNTIME_ROLE" \
     -c 'GRANT USAGE ON SCHEMA rustodon TO :"runtime_role";
         GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE
           rustodon.durable_jobs, rustodon.outbox_events,
           rustodon.idempotency_keys, rustodon.ordering_markers,
           rustodon.domain_health, rustodon.heartbeats,
           rustodon.rate_limit_windows TO :"runtime_role";
         GRANT SELECT, INSERT, DELETE ON TABLE
           rustodon.remote_fetch_leases TO :"runtime_role";
         GRANT SELECT ON TABLE rustodon.schema_migrations TO :"runtime_role";
         GRANT USAGE ON SEQUENCE
           rustodon.durable_jobs_id_seq, rustodon.outbox_events_id_seq
           TO :"runtime_role";'
   ```

   An upgrade with an already-discovered runtime role grants only newly
   introduced operational-table privileges transactionally; retain this full
   grant step for fresh roles and verify it for every deployment.

   The writer must have no membership, ownership, grant options, public object
   grants, or privileges in schemas outside `public` and `rustodon`. Its table,
   sequence, column, and function grants must match the preflight contract.
6. Run `rustodon preflight` again with the complete production environment,
   including `WRITE_DATABASE_URL`. It must finish successfully before starting
   either Rustodon process.
7. Start the worker with the production environment. Wait for
   `rustodon admin worker-readiness` to report `ready=true`.
8. Start the web process. Confirm `/health` and `/ready` before enabling the
   proxy. Keep the existing Nginx paths, TLS termination, trusted-proxy
   settings, and ActivityPub host headers unchanged.
9. Keep Redis available during the rollback window. Rustodon does not consume
   old Sidekiq work; remove Redis only after the old queues are drained and the
   operator has accepted the Rustodon smoke result. If Mastodon is restored,
   provide a fresh empty Redis service rather than relying on Rustodon's retired
   operational keys; the fixture cutover rehearsal proves this replacement path.

## Non-Destructive Smoke Test

Use a real bearer token belonging to an existing local account and one known
local media URL. The empty marker POST authenticates the configured writer path
but submits no timeline values, so it does not update a marker row.

```console
RUSTODON_SMOKE_TOKEN='read-write-token' \
  tools/rustodon-smoke \
  https://example.social \
  116844606259201001 \
  alice \
  https://example.social/system/accounts/avatars/000/000/001/original/avatar.png
```

The command checks:

- `/health` and `/ready` for process and worker readiness;
- instance, account, status, and marker reads;
- bearer authentication and the no-op marker write;
- one local Paperclip media file through `HEAD`;
- WebFinger and ActivityPub actor discovery with the production host.

Do not put the bearer token directly in the command line. The script writes the
environment value to a mode-restricted temporary curl header file and removes
it on exit, avoiding exposure through process arguments. Shell history can also
be disabled or cleared according to the operator's secret-handling policy.

## Acceptance And Monitoring

After the smoke test, enable normal traffic and perform one manual check each
for password/TOTP login, an existing OAuth client, a public status read, a
private/direct status read, an image read, a notification, and a streaming
connection. Watch `/ready`, worker readiness, dead letters, PostgreSQL errors,
media errors, and ActivityPub delivery/domain health for the entire rollback
window.

Do not delete the PostgreSQL snapshot or the original media copy until the
rollback window closes. Rustodon's operational rows and outbox events may be
kept for diagnostics; they are not required by Mastodon.

## Rollback

Rollback immediately for schema/readiness failures, private-content exposure,
deleted-object resurrection, repeated worker dead letters, unexplained media
loss, or inability to authenticate existing users/OAuth clients.

1. Stop proxy traffic and stop Rustodon web and worker processes. Do not let
   Mastodon and Rustodon write concurrently.
2. Preserve Rustodon logs, readiness output, dead-job metadata, and the
   operational schema for investigation. Do not run a destructive cleanup
   against Mastodon-owned tables.
3. Restore the Nginx upstream and environment to Mastodon, including the same
   domain, PostgreSQL database, media root, and persistent secrets.
4. Start Mastodon web, streaming, and worker processes. Keep the Sidekiq queues
   drained before allowing writes, unless the operator has explicitly chosen a
   documented recovery procedure for the captured Rustodon effects.
5. Verify login, an existing OAuth token, public/private/direct reads, media,
   and the ActivityPub actor/status endpoints through Mastodon.
6. Re-run `rustodon preflight` against the restored state before scheduling a
   new cutover. A Rustodon operational schema can remain in PostgreSQL because
   it is isolated and Mastodon ignores it.

The rollback path requires no reversal of Mastodon schema migrations: Rustodon
uses existing Mastodon tables and adds only isolated operational state.
