#!/bin/sh
# Called only by the fixed differential adapter after fresh PG/Rust setup.
set -eu
umask 077
W=/srv/workspaces/rustodon-hashtag-differential-bd01aca-alice
P=hashtag-differential-bd01aca
cd "$W"
test "$(cat evidence/application-revision.txt)" = bd01acae2bc4e1b8a75bd95648e216535c790330
. harness/tools/hashtag-runtime/rails-functions.sh
run_podman() { podman "$@"; }
die() { printf '%s\n' "$*" >&2; exit 1; }
PG_CONTAINER="$P-pg-alice"
NETWORK="$P-alice"
REDIS_CONTAINER="$P-redis-alice"
BROWSER=d6337b96fb60b14e3bbd27ead60d4a781ef4ef3317c3f610b4ec4b21b655c094
podman image exists "$MASTODON_IMAGE"
podman image exists "$REDIS_IMAGE"
podman image inspect "$MASTODON_IMAGE" "$REDIS_IMAGE" --format '{{.Id}} {{.RepoDigests}}' > evidence/oracle-image-identities.txt
# Rails is a different owned DB/media root; no broad grants on the Rust DB.
podman exec -i "$PG_CONTAINER" psql -U postgres -d postgres -v ON_ERROR_STOP=1 > evidence/rails-database-setup.txt <<SQL
CREATE ROLE $DATABASE_USER LOGIN NOINHERIT NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS PASSWORD '$DATABASE_PASSWORD';
CREATE DATABASE hashtag_rails OWNER $DATABASE_USER;
COMMENT ON DATABASE hashtag_rails IS 'task-owned hashtag differential bd01aca rails';
COMMENT ON DATABASE remote_browser IS 'task-owned hashtag differential bd01aca rust';
SQL
podman exec -i "$PG_CONTAINER" psql -U "$DATABASE_USER" -d hashtag_rails -v ON_ERROR_STOP=1 < source/fixtures/mastodon/v4.6.5/database.sql > evidence/rails-restore.log
podman exec -i "$PG_CONTAINER" psql -U "$DATABASE_USER" -d hashtag_rails -v ON_ERROR_STOP=1 < harness/tools/hashtag-controls-browser/differential-seed.sql > evidence/rails-seed.log
mkdir rails-media
cp -a source/fixtures/mastodon/v4.6.5/media/. rails-media/
chmod -R a+rwX rails-media
podman run -d --pull=never --name "$REDIS_CONTAINER" --network "$NETWORK" --cpus=1 --memory=256m --memory-swap=256m --pids-limit=64 --timeout=600 --tmpfs /data:rw,nosuid,nodev,noexec,size=64m "$REDIS_IMAGE" redis-server --save '' --appendonly no
# Existing launch function, pinned environment; only safety bounds/no publish adapted.
# false disables unrelated feed preparation. No Sidekiq or Rust workers start.
start_differential_web hashtag_rails "$W/rails-media" rw false
podman exec "$PG_CONTAINER" psql -U postgres -d postgres -c "SELECT datname,pg_get_userbyid(datdba) AS owner FROM pg_database WHERE datname IN ('remote_browser','hashtag_rails');" -c "SELECT rolname,rolsuper,rolcreatedb,rolcreaterole,rolbypassrls,(SELECT count(*) FROM pg_auth_members WHERE member=r.oid) memberships FROM pg_roles r WHERE rolname IN ('browser_runtime','browser_writer','rustodon_fixture');" > evidence/differential-role-proof.txt
# Bounded real HTTP driver inside task network. Credentials remain in fixture source,
# and are not included in serialized request/response evidence.
timeout -k 10 240 podman run --rm --pull=never --name "$P-compare-alice" --network "container:$PG_CONTAINER" --read-only --cap-drop=all --security-opt=no-new-privileges --cpus=1 --memory=256m --memory-swap=256m --pids-limit=128 --timeout=220 --tmpfs /tmp:size=64m -v "$W/harness:/harness:ro" -v "$W/evidence:/evidence:rw" "$BROWSER" python3 /harness/tools/hashtag-controls-browser/differential.py
