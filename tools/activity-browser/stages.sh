#!/bin/sh
# Run in the fixed task workspace by the existing bounded launcher.
set -eu
P=activity-browser-10ddf66
PG=$P-pg-alice
ui() { timeout 150 podman exec "$P-browser-alice" python3 /harness/tools/activity-browser/ui.py "$1"; }
sql() { podman exec -i "$PG" psql -U postgres -d remote_browser -v ON_ERROR_STOP=1; }
restart() {
  podman restart "$P-web-alice" >/dev/null
  timeout 75 podman exec "$P-browser-alice" python3 /harness/tools/activity-runtime/ready.py
}
# Exact runtime/writer role attach/grants evidence, not just role flags.
sql > evidence/grant-attach-proof.txt <<'SQL'
SELECT usename, application_name, count(*) FROM pg_stat_activity WHERE datname=current_database() GROUP BY 1,2;
SELECT grantee,table_schema,table_name,privilege_type FROM information_schema.role_table_grants WHERE grantee IN ('browser_runtime','browser_writer') ORDER BY 1,2,3,4;
DO $$ BEGIN
 IF (SELECT array_agg(version ORDER BY version) FROM rustodon.schema_migrations) <> ARRAY[1,2,3,4,5,6]::bigint[] THEN RAISE EXCEPTION 'wrong migrations'; END IF;
 IF EXISTS(SELECT 1 FROM rustodon.activity_members) OR EXISTS(SELECT 1 FROM rustodon.activity_buckets) THEN RAISE EXCEPTION 'nonempty baseline'; END IF;
 IF has_table_privilege('browser_runtime','rustodon.activity_members','INSERT') OR NOT has_table_privilege('browser_writer','rustodon.activity_members','INSERT') THEN RAISE EXCEPTION 'wrong activity grants'; END IF;
END $$;
SQL
ui baseline
ui login
sql > evidence/today-membership.txt <<'SQL'
SELECT b.day,b.expires_at,m.user_id,u.confirmed_at IS NOT NULL AS confirmed,u.approved,a.username FROM rustodon.activity_buckets b JOIN rustodon.activity_members m USING(day) JOIN users u ON u.id=m.user_id JOIN accounts a ON a.id=u.account_id;
DO $$ BEGIN
 IF (SELECT count(*) FROM rustodon.activity_members WHERE day=(clock_timestamp() AT TIME ZONE 'UTC')::date) <> 1 THEN RAISE EXCEPTION 'real login did not record exactly one today member'; END IF;
END $$;
SQL
# Seeded simulated history: MOVE the bucket/member created by the real login helper.
# No direct INSERT, no actual historical-backfill or observed-rollover claim.
sql > evidence/simulated-history.txt <<'SQL'
BEGIN;
UPDATE rustodon.activity_members SET day=day-1;
UPDATE rustodon.activity_buckets SET day=day-1;
SELECT b.day,b.expires_at,m.user_id,(clock_timestamp() AT TIME ZONE 'UTC')::date AS setup_today FROM rustodon.activity_buckets b JOIN rustodon.activity_members m USING(day);
COMMIT;
SQL
restart
ui historical
# Same binary with native limited-mode config; no app test endpoint or clock patch.
podman stop "$P-web-alice" >/dev/null
podman logs "$P-web-alice" > evidence/ordinary-web.log 2>&1
podman rm "$P-web-alice" >/dev/null
W=$PWD
podman run -d --name "$P-web-alice" --pull=never --network container:"$PG" --cpus=2 --memory=1g --memory-swap=1g --pids-limit=256 --timeout=900 --read-only --cap-drop=all --security-opt=no-new-privileges --tmpfs /tmp:size=256m --env-file run-fixture/app.env -e LIMITED_FEDERATION_MODE=true -v "$W/source:/workspace:ro" -v "$W/evidence/rustodon:/rustodon:ro" -v "$W/run-fixture:/run-fixture:ro" -v "$W/media:/media:rw" 7203e0222e2bb72e0b83ab623051873604874cc41a688e318b84691bfa77ad8a /rustodon web >/dev/null
timeout 75 podman exec "$P-browser-alice" python3 /harness/tools/activity-runtime/ready.py
ui limited
