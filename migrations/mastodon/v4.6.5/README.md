# Standalone Mastodon-compatible schema

This directory contains the empty PostgreSQL schema used to bootstrap a fresh
Rustodon instance without installing or running Mastodon. It targets Mastodon
v4.6.5 revision `1440d55b139e39ec722c2a3db7f60b66cd889048` and PostgreSQL
14.23. It is not the populated compatibility fixture and contains no account or
content rows, keys, tokens, domains, current sequence values, object ownership,
object comments, or grants. Ordinary `pg_dump` explanatory comments and sequence
definitions remain.

## Provenance

Mastodon is distributed under the GNU Affero General Public License v3.0. This
schema is a generated derivative of the pinned Mastodon source schema. The exact
corresponding source is obtained with `mise run fixture-obtain` and verified by
`mise run pinned-source-contracts`. See Mastodon's `LICENSE` at the pinned
revision for its terms. This notice records provenance and is not legal advice.

The source schema was loaded by the repository's pinned Mastodon fixture into
its pinned PostgreSQL 14 image. The schema-only dump was produced with:

```console
pg_dump --format=plain --encoding=UTF8 --no-owner --no-privileges \
  --no-comments --schema=public --schema-only DATABASE
```

`tools/standalone-schema-artifact normalize` then performs only these narrow
normalizations:

1. remove the single matching PostgreSQL `\\restrict`/`\\unrestrict` pair;
2. omit the dump's `CREATE SCHEMA public` block because a conventional fresh
   database already has an empty `public` schema;
3. replace the fixture's deterministic `timestamp_id()` salt with the single
   `__RUSTODON_TIMESTAMP_ID_SALT__` installer sentinel;
4. remove trailing blank lines and retain one terminal newline;
5. reject data statements, other psql commands, fixture identities, missing
   required schema objects, or any result that differs from the independently
   pinned byte length and SHA-256.

Regenerate both the SQL and manifest from a fresh verified schema-only dump:

```console
tools/standalone-schema-artifact normalize RAW_SCHEMA_DUMP \
  migrations/mastodon/v4.6.5
```

The installer must first verify that the existing `public` schema is empty. It
executes this multi-statement artifact through `sqlx::raw_sql` on a dedicated
connection that is discarded afterward, because pg_dump's session-local `SET`
statements are intentionally retained. Never edit `public-schema.sql` or its
manifest by hand.

## Verification

The offline contract is included in the ordinary harness lane:

```console
python3 tools/tests/standalone-schema-artifact-test.py
mise run harness-tests
```

The installer replaces the salt sentinel in memory with fresh random
hexadecimal data before executing the SQL. Baseline migration rows, roles,
settings, and instance identities are deliberately provisioned separately with
bound SQL; none belong in this artifact.
