#!/bin/sh
# Focused gate controller, after setup and task-local TLS/browser login.
# Run on NAS: timeout -k 10 600 sh harness/tools/remote-browser/accept.sh
set -eu
W=/srv/workspaces/rustodon-remote-browser-8b2f49e-r2-alice
PG=remote-browser-8b2f49e-r2-pg-alice
B=remote-browser-8b2f49e-r2-browser-alice
P=remote-browser-8b2f49e-r2-pull-alice
cd "$W"
ab() { podman exec "$B" sh /harness/tools/remote-browser/browser "$@"; }
ui() { podman exec "$B" python3 /harness/tools/remote-browser/ui.py "$@"; }
sql() { podman exec "$PG" psql -U postgres -d remote_browser -v ON_ERROR_STOP=1 "$@"; }
test "$(sql -Atc "SELECT count(*) FROM statuses WHERE uri LIKE 'https://remote.fixture.invalid/notes/remote-browser-8b2f49e-r2-final-%'")" = 0
ab open https://fixture-v4-6-5.rustodon.invalid/home
ab wait --fn 'window.remoteBrowserEvents.some(e=>e.event==="open")'
ui network startup
for kind in video audio still; do
    podman stop -t 5 "$P"
    podman exec "$B" python3 /harness/tools/remote-browser/fixture.py deliver "$kind" > "evidence/$kind-delivery.json"
    query="SELECT json_build_object('status',s.id::text,'media',m.id::text) FROM statuses s JOIN media_attachments m ON m.status_id=s.id WHERE s.uri='https://remote.fixture.invalid/notes/remote-browser-8b2f49e-r2-final-$kind'"
    n=0
    while :; do
        sql -Atc "$query" > "evidence/$kind-identity.json"
        test ! -s "evidence/$kind-identity.json" || break
        n=$((n+1)); test "$n" -lt 30; sleep 1
    done
    sql -c "SELECT s.id,m.id,j.kind,j.attempts,j.lease_owner,m.processing,m.file_file_name FROM statuses s JOIN media_attachments m ON m.status_id=s.id JOIN rustodon.durable_jobs j ON (j.arguments->>'media_id')::bigint=m.id WHERE s.uri='https://remote.fixture.invalid/notes/remote-browser-8b2f49e-r2-final-$kind'" > "evidence/$kind-held-worker.txt"
    test "$(sql -Atc "SELECT count(*) FROM statuses s JOIN media_attachments m ON m.status_id=s.id JOIN rustodon.durable_jobs j ON (j.arguments->>'media_id')::bigint=m.id WHERE s.uri='https://remote.fixture.invalid/notes/remote-browser-8b2f49e-r2-final-$kind' AND j.kind='rustodon.activitypub.fetch_media' AND j.attempts=0 AND j.lease_owner IS NULL AND m.processing=0 AND m.file_file_name IS NULL")" = 1
    ui pending "$kind"
    touch "run-fixture/release-$kind"
    podman start "$P"
    ui updated "$kind"
    ui verify "$kind" live
    ui network "$kind-live" "$kind"
done
for kind in video audio still; do
    id=$(python3 -c "import json; print(json.load(open('evidence/$kind-identity.json'))['status'])")
    ab open "https://fixture-v4-6-5.rustodon.invalid/@bob@remote.fixture.invalid/$id"
    ab wait --fn "window.remoteBrowserRest.some(r=>r.id==='$id')"
    ui network "$kind-navigation"
    ab reload
    ui verify "$kind" reload
    ui network "$kind-reload" "$kind"
done
printf '%s\n' 'PASS: uninterrupted remote rich-media controller'
