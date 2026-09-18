#!/bin/sh
# Opt-in NAS-only disposable setup. Run after reviewed source/build provenance.
set -eu
umask 077
W=/srv/workspaces/rustodon-remote-browser-8b2f49e-r2-alice
N=remote-browser-8b2f49e-r2-alice
PG=remote-browser-8b2f49e-r2-pg-alice
TOOLS=7203e0222e2bb72e0b83ab623051873604874cc41a688e318b84691bfa77ad8a
cd "$W"
test "$(cat evidence/application-revision.txt)" = 8b2f49e42a7cddd75b9d2dce4be24cfcd388099c
mkdir run-fixture media
cp -a source/fixtures/mastodon/v4.6.5/media/. media/
(cd source && sha256sum -c public/packs/SHA256SUMS) > evidence/frontend-integrity.txt
podman network create --internal "$N" > evidence/network-id.txt
podman run -d --name "$PG" --pull=never --network "$N" --cpus=1 --memory=512m --memory-swap=512m --pids-limit=128 --timeout=5400 -e POSTGRES_PASSWORD=browser-fixture -e POSTGRES_DB=remote_browser 1a6c2409ab71f4d054d676ba09d9b74b5d843d805bd5a0cf08314d27ca659d37
n=0; until podman exec "$PG" psql -h 127.0.0.1 -U postgres -d remote_browser -c 'SELECT 1' >/dev/null 2>&1; do n=$((n+1)); test "$n" -lt 60; sleep 1; done
podman exec -i "$PG" psql -U postgres -d remote_browser -v ON_ERROR_STOP=1 < source/fixtures/mastodon/v4.6.5/database.sql > evidence/restore.log
podman exec -i "$PG" psql -U postgres -d remote_browser -v ON_ERROR_STOP=1 > evidence/roles.log <<'SQL'
CREATE ROLE browser_runtime LOGIN NOINHERIT PASSWORD 'runtime-fixture' NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS;
CREATE ROLE browser_writer LOGIN NOINHERIT PASSWORD 'writer-fixture' NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS;
SQL
cat > run-fixture/app.env <<'ENV'
LOCAL_DOMAIN=fixture-v4-6-5.rustodon.invalid
WEB_DOMAIN=fixture-v4-6-5.rustodon.invalid
DATABASE_URL=postgres://browser_runtime:runtime-fixture@127.0.0.1/remote_browser
WRITE_DATABASE_URL=postgres://browser_writer:writer-fixture@127.0.0.1/remote_browser
DB_SSLMODE=disable
DB_POOL=4
PAPERCLIP_ROOT_PATH=/media
PAPERCLIP_ROOT_URL=/system
TRUSTED_PROXY_IP=127.0.0.1/32,::1/128
SECRET_KEY_BASE=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
ACTIVE_RECORD_ENCRYPTION_DETERMINISTIC_KEY=11111111111111111111111111111111
ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT=22222222222222222222222222222222
ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY=33333333333333333333333333333333
WORKER_LANES=ingress
WORKER_CONCURRENCY=1
WORKER_HEARTBEAT_SECONDS=1
WORKER_POLL_MILLISECONDS=25
WORKER_SHUTDOWN_SECONDS=5
BIND=127.0.0.1
PORT=18374
RUSTODON_TEST_PEER_ORIGINS={"https://remote.fixture.invalid":"127.0.0.1:19443"}
RUSTODON_TEST_PEER_CA=/run-fixture/remote.pem
ENV
timeout 60 podman run --rm --name remote-browser-8b2f49e-r2-migrate-alice --pull=never --network container:"$PG" --cpus=1 --memory=512m --pids-limit=128 --timeout=50 --read-only --cap-drop=all --security-opt=no-new-privileges --env-file run-fixture/app.env -e DATABASE_URL=postgres://postgres:browser-fixture@127.0.0.1/remote_browser -v "$W/source:/workspace:ro" -v "$W/evidence/rustodon:/rustodon:ro" -v "$W/media:/media:rw" "$TOOLS" /rustodon admin migrate-operational-schema > evidence/migrate.log 2>&1
# Extract the literal runtime grant policy from exact pinned bootstrap source.
python3 - <<'PY' > run-fixture/runtime-grants.sql
from pathlib import Path
s=Path('source/src/bootstrap.rs').read_text().split('async fn apply_runtime_grants(',1)[1].split('sqlx::raw_sql',1)[0]
s=s.split('"REVOKE ALL PRIVILEGES',1)[1].rsplit('"',1)[0]
print(('REVOKE ALL PRIVILEGES'+s).replace('\\\n','').replace('{database}','"remote_browser"').replace('{role}','"browser_runtime"'))
PY
for SQL in source/docs/mastodon-refresh-instances.sql run-fixture/runtime-grants.sql; do podman exec -i "$PG" psql -U postgres -d remote_browser -v ON_ERROR_STOP=1 < "$SQL" >> evidence/grants.log; done
podman exec -i "$PG" psql -U postgres -d remote_browser -v ON_ERROR_STOP=1 -v writer_role=browser_writer < source/docs/mastodon-writer-grants.sql >> evidence/grants.log
openssl req -x509 -newkey rsa:2048 -nodes -days 1 -subj /CN=remote-browser-ca -keyout run-fixture/ca.key -out run-fixture/remote.pem 2>/dev/null
openssl req -new -newkey rsa:2048 -nodes -subj /CN=remote.fixture.invalid -keyout run-fixture/remote.key -out run-fixture/leaf.csr 2>/dev/null
printf '%s\n' 'subjectAltName=DNS:remote.fixture.invalid' 'basicConstraints=critical,CA:FALSE' 'extendedKeyUsage=serverAuth' > run-fixture/leaf.ext
openssl x509 -req -in run-fixture/leaf.csr -CA run-fixture/remote.pem -CAkey run-fixture/ca.key -CAcreateserial -days 1 -extfile run-fixture/leaf.ext -out run-fixture/remote-leaf.pem 2>/dev/null
openssl genrsa -out run-fixture/actor.key 2048 2>/dev/null
openssl pkey -in run-fixture/actor.key -pubout -out run-fixture/actor.pub 2>/dev/null
python3 - <<'PY' | podman exec -i "$PG" psql -U postgres -d remote_browser -v ON_ERROR_STOP=1 > evidence/actor-setup.log
from pathlib import Path
key=Path('run-fixture/actor.pub').read_text()
print("UPDATE accounts SET public_key=$key$"+key+"$key$, last_webfingered_at=clock_timestamp() WHERE id=116844606259202001;")
# Baseline Alice both mutes Bob and routes him exclusively to a list. This
# controlled fixture needs ordinary home-stream eligibility, not a policy test.
print("DELETE FROM mutes WHERE account_id=116844606259201001 AND target_account_id=116844606259202001;")
print("UPDATE lists SET exclusive=false WHERE account_id=116844606259201001 AND id IN (SELECT list_id FROM list_accounts WHERE account_id=116844606259202001);")
PY
for mode in web worker; do
podman run -d --name remote-browser-8b2f49e-r2-"$mode"-alice --pull=never --network container:"$PG" --cpus=2 --memory=1g --memory-swap=1g --pids-limit=256 --timeout=5000 --read-only --cap-drop=all --security-opt=no-new-privileges --tmpfs /tmp:size=256m --env-file run-fixture/app.env -v "$W/source:/workspace:ro" -v "$W/evidence/rustodon:/rustodon:ro" -v "$W/run-fixture:/run-fixture:ro" -v "$W/media:/media:rw" "$TOOLS" /rustodon "$mode"
done
podman exec "$PG" psql -U postgres -d remote_browser -c "SHOW server_version" -c "SELECT rolname,rolsuper,rolcreaterole,rolcreatedb,rolbypassrls, (SELECT count(*) FROM pg_auth_members WHERE member=r.oid) memberships FROM pg_roles r WHERE rolname IN ('browser_runtime','browser_writer');" > evidence/role-proof.txt
