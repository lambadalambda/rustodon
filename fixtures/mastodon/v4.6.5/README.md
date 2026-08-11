# Mastodon v4.6.5 fixture

This directory is an immutable, release-versioned compatibility fixture for
Mastodon v4.6.5. `manifest.json` is the machine-readable index. `database.sql`
is a normalized full PostgreSQL dump, `migrations.tsv` is the complete checked
migration inventory, and `catalog.txt` is the complete structural catalog used
to produce the catalog SHA-256 fingerprint.

The dump is intended to be restored with PostgreSQL 14. It contains the full
Mastodon schema and deterministic fixture rows, including migration and
sequence state. Mastodon Rails and future Rust tests can read the same restored
database and the `media/` Paperclip tree.

All seven `timestamp_id()` backing sequences use invocation counts rather than
object IDs. Fixture Snowflake IDs encode each row's explicit `created_at`
millisecond epoch in their upper 48 bits; SQL and Rails verification decode
every account, status, media attachment, quote, collection, collection item,
and notification request ID and check sequence state.

The fixture includes a local instance actor with signed ID `-99` and no user,
an unavailable local user, NULL/empty/populated user arrays and raw JSON
settings, nullable and populated account arrays, and account JSONB data. It
covers all five known status visibilities plus a soft-deleted unknown value,
status history, tag joins, featured tags, a direct conversation and mute, and
the v1 relationship and domain-policy records. Deleted-status associations are
present across ordinary Rust read paths so status suppression and retention of
live parent records are both testable.
Notification data retains the original 17 known checks and adds filtered
unknown and NULL types, a deleted known activity, plus policy, permission,
request, and activity-target rows.
It also covers scalar/tagged Rails YAML without normalization, a tombstone, and
a valid deterministic Active Record encrypted local keypair value that Rust
treats as opaque. Existing quote, collection, collection-item, poll, remote
public keypair, OAuth, and media records remain part of the 4.6.5 baseline.

## Test identities

All domains use the reserved `.invalid` suffix. The local instance domain is
`fixture-v4-6-5.rustodon.invalid`; remote actors use
`remote.fixture.invalid`. The password for all three local users is
`fixture-password`. The OAuth bearer token is
`fixture-bearer-token-v4-6-5`. These values are public, test-only material and
must never be reused outside an isolated fixture database.

The RSA key is the published Mastodon test key from
`spec/requests/signature_verification_spec.rb` at the pinned revision. Local
accounts retain that key in both `accounts.private_key` and
`accounts.public_key`, matching Mastodon 4.6.5's signing-key contract. Remote
accounts have no private key. The remote `keypairs` row is also public-only.

The avatar source is Mastodon's published `spec/fixtures/files/emojo.png`; the
status image source is `spec/fixtures/files/attachment.jpg`. The generator
extracts both from pinned Git blobs and runs
`bundle exec rails runner /fixture/process-media.rb` in the pinned Mastodon
image. That invokes the v4.6.5 Paperclip/libvips processors and writes a 400x400
avatar, 600x400 media original, and 588x392 media small style with deterministic
obfuscated names. Database file metadata, completed processing state, blurhash,
output dimensions, source/output hashes, and bytes are verified and recorded.

## Rebuilding

Run from the Rustodon repository root:

```console
mise run fixture-obtain
mise run fixture-generate
mise run fixture-verify
mise run fixture-restore-verify
mise run fixture-repro
mise run mastodon-schema-integration
mise run differential
mise run differential -- instance_v2
```

`fixture-verify` is fast and does not require Podman. The generate,
restore/verify, byte-for-byte reproducibility, read-only Rust integration, and
differential tasks are separate expensive Podman tasks. The generator does not load `.env`
files or pass inherited database, domain, or secret variables into containers.
Its database and
domains are constants, and `seed.sql` aborts unless connected to the dedicated
fixture database.

The Podman tasks require GNU/Linux x86-64, Git, GNU core utilities, and Podman
5.x. The Mastodon, PostgreSQL, and Redis OCI index digests and resolved
`linux/amd64` child manifest digests are pinned in `manifest.json`; every
container is forced to that platform.
Bind mounts use Podman's private `Z` SELinux relabeling. The source verifier
rejects tracked or untracked changes, and migrations/media are read from the
pinned commit's Git blobs. PostgreSQL uses a labeled named volume that is
removed with the container and checked absent after each successful task.

`fixture-restore-verify` has no database-name, host, or domain override: it
creates and destroys its own dedicated container database, and both the tool
and `verify.sql` enforce `rustodon_mastodon_v4_6_5_fixture`. Do not manually pipe
`database.sql` into an operator database; the supported restore command is the
guarded fixture task above. Redis is not started for direct Paperclip processing
or Rails model verification. The live HTTP differential task starts its own
empty, pinned Redis because Mastodon's production cache and rate limiter require
it; Sidekiq and streaming remain disabled.

`mastodon-schema-integration` restores the checked dump into an isolated
container published on a random `127.0.0.1` port. It creates a non-owner LOGIN
role with only database `CONNECT`, public-schema `USAGE`, and table `SELECT`;
the role has no sequence, DML, truncation, or schema-creation privileges and
defaults to read-only transactions. The ignored Rust test deliberately turns
that session default off and confirms PostgreSQL privileges still reject every
mutation before checking that fixture rows are unchanged.

`differential` template-clones the verified database into separately marked
Mastodon and Rust databases, applies the same checked deterministic setting
overlay to both, and copies the media tree into separately marked roots. The
Rust comparator receives only loopback HTTP URLs and SELECT-only database
credentials. It rejects mismatched database comments, paths outside its exact
`target/differential-<run-id>/` root, symlinks, and media trees without exact
side markers before sending a request. Cleanup checks that the labeled
PostgreSQL volume and marked run root are gone.

The only four narrow normalizations address known nondeterminism and terminal
dump formatting: Mastodon's schema loader creates `timestamp_id()` with a random
salt, Rails timestamps its two `ar_internal_metadata` rows at load time,
PostgreSQL 14.23 emits a random `\\restrict`/`\\unrestrict` token, and `pg_dump`
emits trailing empty lines. The seed fixes the function salt and internal
metadata timestamps before dumping and fingerprinting; the dump stream replaces
only those two token lines with `rustodonMastodon465FixtureDump`, removes only
trailing empty `pg_dump` lines, and enforces exactly one terminal LF while
preserving all internal blank lines. The pinned `pg_dump` is invoked with owner,
ACL, and comments excluded; no other broad textual filtering is performed.

## Updating the baseline

Never modify this directory to represent a later Mastodon release. Copy the
tooling inputs into a new `fixtures/mastodon/vX.Y.Z/` directory, pin the new
tag, commit, schema hash, OCI digest, and PostgreSQL image, then regenerate and
review the migration and catalog diffs. Add a version-specific verification
contract before accepting the new artifacts. Existing release directories
remain unchanged so compatibility behavior cannot move silently.
