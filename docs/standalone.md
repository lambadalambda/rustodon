# Standalone Rustodon Setup

This runbook creates a new, local-media Rustodon instance in an **empty
PostgreSQL 14 database**. It does not install or run Mastodon, Rails, Sidekiq,
or Redis. It is intentionally scoped to one PostgreSQL database and one local
media root.

For an existing Mastodon 4.6.5 database, do not use this procedure. Follow the
separate [cutover and rollback runbook](cutover.md); standalone bootstrap is not
a migration or repair command.

## Prerequisites

- the Rustodon binary built from the intended release;
- `ffmpeg` and `ffprobe` on `PATH`, built with the AVIF/HEIC decoders and
  H.264 (`libx264`), AAC, PNG, JPEG, and MP3 (`libmp3lame`) encoders used by
  Rustodon's bounded media pipeline; startup and `rustodon preflight` exercise
  these exact paths and fail closed when any capability is unavailable;
- PostgreSQL 14, with an administrator able to create one database and three
  login roles;
- an existing, absolute, empty, non-symlink local-media directory writable by
  the Rustodon processes;
- a public domain, TLS reverse proxy, and durable storage for PostgreSQL and the
  media directory;
- a secret manager or a mode-`0600` environment file.

The installer connection must directly own both the database and its `public`
schema. The runtime and writer logins must be distinct, unprivileged, have no
memberships or owned objects, and have no role/database settings. The runtime
login must be `NOINHERIT`. Bootstrap deliberately does not create roles or
rotate their passwords.

## 1. Provision the empty database and roles

Choose deployment-specific role and database names. The following is a shape,
not a script to paste with placeholder passwords:

```sql
CREATE ROLE rustodon_installer LOGIN NOINHERIT
  NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS;
CREATE ROLE rustodon_runtime LOGIN NOINHERIT
  NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS;
CREATE ROLE rustodon_writer LOGIN NOINHERIT
  NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS;
CREATE DATABASE rustodon_production OWNER rustodon_installer;
\connect rustodon_production
ALTER SCHEMA public OWNER TO rustodon_installer;
```

Set each login password interactively with `psql`'s `\password ROLE` or through
your secret manager. Do not put passwords in SQL files, shell history, process
arguments, or the bootstrap command line. Do not grant either application role
anything before bootstrap; the installer applies and verifies the exact
runtime/writer contracts transactionally.

## 2. Prepare durable configuration and media

Create the media directory before bootstrap and leave it empty:

```console
install -d -m 0750 -o rustodon -g rustodon /srv/rustodon/system
```

Store configuration outside the repository in a mode-`0600` secret file. At a
minimum, configuration parsing and bootstrap require values shaped like these:

```sh
LOCAL_DOMAIN=example.social
PAPERCLIP_ROOT_PATH=/srv/rustodon/system
PAPERCLIP_ROOT_URL=/system
DATABASE_URL=postgresql://rustodon_installer:REDACTED@db.example/rustodon_production
SECRET_KEY_BASE=RETAIN_A_RANDOM_SECRET
ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY=RETAIN_A_DISTINCT_RANDOM_SECRET
ACTIVE_RECORD_ENCRYPTION_DETERMINISTIC_KEY=RETAIN_A_DISTINCT_RANDOM_SECRET
ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT=RETAIN_A_DISTINCT_RANDOM_SECRET
```

Generate independent high-entropy values with the deployment secret manager
(or, for example, `openssl rand -hex 32`) and retain them for the life of the
instance. Configure PostgreSQL TLS verification appropriate to the deployment.
Do not reuse the test passwords or `.invalid` domains from the integration
lane.

The media directory must already exist and be empty. Bootstrap will fail closed
if it is missing, is a symlink, or contains any entry. It creates no filesystem
state, so a database failure cannot leave a half-created media tree.

## 3. Bootstrap the instance

Load the protected environment, then run the installer. Omit `--password` so
the first Owner password is read from stdin rather than exposed in the process
list:

```console
rustodon admin bootstrap-instance \
  --admin-username alice \
  --admin-email alice@example.social \
  --site-title 'Example Social' \
  --runtime-role rustodon_runtime \
  --writer-role rustodon_writer
```

Use a strong unique password at the prompt. On success, the command installs the
pinned Mastodon 4.6.5-compatible empty schema, exact migration ledger, baseline
roles/settings, fresh instance and Owner signing keys, Rustodon operational
schema, and least-privilege grants in one database transaction.

An immediate rerun with exactly the same arguments performs a read-only exact
verification and does not rotate keys or the password hash. Once any status,
media, token, or other non-baseline row exists, rerunning bootstrap is expected
to fail. It is not an upgrade or repair tool.

## 4. Switch to application credentials and start

Replace the installer URL in the service environment with the restricted
runtime URL and add the separately restricted writer URL:

```sh
DATABASE_URL=postgresql://rustodon_runtime:REDACTED@db.example/rustodon_production
WRITE_DATABASE_URL=postgresql://rustodon_writer:REDACTED@db.example/rustodon_production
```

Remove installer credentials from the web and worker environment. Retain them
offline only if your operational policy needs the database owner for controlled
maintenance. Before accepting traffic, run:

```console
rustodon preflight
```

Then start one worker and wait for its heartbeat before starting the web
process:

```console
rustodon worker
rustodon admin worker-readiness
rustodon web
```

In production, supervise the two long-running commands separately. Put the web
listener behind a TLS reverse proxy that preserves the public host and forwards
only trusted proxy headers. Configure SMTP before relying on email delivery;
SMTP is optional for bootstrap itself.

Check locally and through the public proxy:

```console
curl --fail http://127.0.0.1:3000/health
curl --fail http://127.0.0.1:3000/ready
curl --fail https://example.social/.well-known/webfinger?resource=acct:alice@example.social
```

`/health` proves the web process is alive. `/ready` additionally depends on the
database and current worker heartbeat; do not route traffic until both pass.
Sign in with the first Owner credentials, upload a small image, publish a public
status, and verify its public and ActivityPub representations.

## Backups and recovery boundaries

Back up all of the following as one documented recovery set:

- PostgreSQL, including both `public` and `rustodon` schemas;
- the complete local-media root, preserving paths, ownership, and modes;
- retained application encryption/session secrets and deployment configuration;
- role credentials or a tested procedure to rotate and update them after a
  restore.

Coordinate database and media snapshots so database rows never reference files
missing from the restored media snapshot. Encrypt backups, restrict access, and
regularly test a restore into an isolated database, media root, roles, domain,
and ports. A schema-only dump does not preserve accounts, signing identities,
statuses, jobs, or media metadata; a database-only backup does not preserve the
media bytes.

Restoring a used instance is a backup/restore operation, not another bootstrap.
Never point bootstrap at a live, partial, drifted, or restored database in the
hope that it will reconcile it; the command intentionally rejects those states.

## Disposable integration lane

On GNU/Linux with rootless Podman and sufficient local resources, the supported
empty-database lane is:

```console
mise run standalone-bootstrap-integration
```

It verifies the pinned PostgreSQL 14 image, creates task-owned roles, database,
loopback port, volume, network, and media root, then exercises installation,
exact rerun verification, fail-closed states, first-Owner browser login, media
upload, public status creation, WebFinger, and ActivityPub. It does not use the
Mastodon fixture or source tree. `mise run harness-tests` checks the runner's
offline wiring and cleanup paths only; it is not a substitute for the live lane.
