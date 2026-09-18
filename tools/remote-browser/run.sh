#!/bin/sh
# NAS-only, fixed task namespace. Caller archives exact source + explicit harness.
# timeout -k 20 1500 sh harness/tools/remote-browser/run.sh
set -eu
umask 077
W=/srv/workspaces/rustodon-remote-browser-8b2f49e-r2-alice
P=remote-browser-8b2f49e-r2
TOOLS=7203e0222e2bb72e0b83ab623051873604874cc41a688e318b84691bfa77ad8a
BROWSER=d6337b96fb60b14e3bbd27ead60d4a781ef4ef3317c3f610b4ec4b21b655c094
cd "$W"
test "$(cat evidence/application-revision.txt)" = 8b2f49e42a7cddd75b9d2dce4be24cfcd388099c
test ! -e run-fixture
for n in build migrate pg web worker pull source tls forward browser; do
    if podman container exists "$P-$n-alice"; then echo "Task name already exists: $n" >&2; exit 1; fi
done
if podman network exists "$P-alice"; then echo 'Task network already exists' >&2; exit 1; fi
trap 'code=$?; trap - EXIT; sh harness/tools/remote-browser/cleanup.sh; exit "$code"' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
mkdir target
# Existing dependency cache only; fresh task-owned output, exact tracked source.
timeout -k 15 900 podman run --rm --name "$P-build-alice" --pull=never --network none --read-only --cap-drop=all --security-opt=no-new-privileges --cpus=4 --memory=6g --memory-swap=6g --pids-limit=512 --timeout=870 --tmpfs /tmp:rw,nosuid,nodev,size=1g -v "$W/source:/workspace:ro" -v /srv/workspaces/rustodon-null-route-20260918/cargo-home:/cargo-home:rw -v "$W/target:/target:rw" -v "$W/evidence:/evidence:rw" -w /workspace -e CARGO_HOME=/cargo-home -e CARGO_TARGET_DIR=/target -e CARGO_BUILD_JOBS=4 -e CARGO_NET_OFFLINE=true -e CC=clang -e AR=/workspace/tools/clang-ar "$TOOLS" sh -c 'cargo build --locked --offline --features test-support --bin rustodon && cp /target/debug/rustodon /evidence/rustodon && sha256sum /evidence/rustodon' > evidence/build.log 2>&1
timeout -k 10 180 sh harness/tools/remote-browser/setup.sh > evidence/setup.log 2>&1
run() {
    name=$1; memory=$2; cpus=$3; shift 3
    podman run -d --name "$P-$name-alice" --pull=never --network "container:$P-pg-alice" --read-only --cap-drop=all --security-opt=no-new-privileges --cpus="$cpus" --memory="$memory" --memory-swap="$memory" --pids-limit=256 --timeout=1200 --tmpfs /tmp:size=512m -v "$W/source:/workspace:ro" -v "$W/harness:/harness:ro" -v "$W/run-fixture:/run-fixture:rw" -v "$W/evidence:/evidence:rw" "$@"
}
run source 256m 1 "$BROWSER" python3 /harness/tools/remote-browser/fixture.py serve
mkdir -m 700 run-fixture/tls
run tls 256m 1 "$BROWSER" python3 /workspace/tools/browser-fixture-tls --directory /run-fixture/tls --domain fixture-v4-6-5.rustodon.invalid --backend-port 18374
n=0; until test -s run-fixture/tls/port; do n=$((n+1)); test "$n" -lt 30; sleep 1; done
run forward 128m 1 --cap-add=NET_BIND_SERVICE "$BROWSER" python3 /harness/tools/remote-browser/forward.py
run browser 2g 2 --shm-size=256m --tmpfs /root:size=128m "$BROWSER" sleep 1150
run pull 1g 2 --env-file run-fixture/app.env -e WORKER_LANES=pull -v "$W/evidence/rustodon:/rustodon:ro" -v "$W/media:/media:rw" "$TOOLS" /rustodon worker
timeout 75 podman exec "$P-browser-alice" python3 /harness/tools/remote-browser/ready.py > evidence/readiness.txt
podman exec "$P-browser-alice" agent-browser skills get core > evidence/browser-core.md
podman exec "$P-browser-alice" agent-browser --version > evidence/browser-version.txt
podman exec "$P-browser-alice" sh /harness/tools/remote-browser/browser open --init-script /harness/tools/remote-browser/observe.js https://fixture-v4-6-5.rustodon.invalid/auth/sign_in
podman exec "$P-browser-alice" python3 /harness/tools/remote-browser/ui.py login
timeout -k 10 600 sh harness/tools/remote-browser/accept.sh > evidence/accept.log 2>&1
podman exec "$P-pg-alice" psql -U postgres -d remote_browser -c "SELECT s.id AS status,m.id AS media,m.processing,m.type,m.file_content_type,m.file_file_name,m.file_meta FROM statuses s JOIN media_attachments m ON m.status_id=s.id WHERE s.uri LIKE 'https://remote.fixture.invalid/notes/remote-browser-8b2f49e-r2-final-%' ORDER BY s.id" -c "SELECT id,payload FROM rustodon.outbox_events WHERE kind='rustodon.mastodon.stream_event' ORDER BY id" > evidence/installed-media-and-stream-events.txt
podman image inspect "$TOOLS" "$BROWSER" 1a6c2409ab71 --format '{{.Id}} {{.RepoTags}}' > evidence/image-identities.txt
printf '%s\n' 'PASS: fresh uninterrupted bounded gate' > evidence/result.txt
