#!/bin/sh
# Removes only resources of the fixed r2 task. Also safe after run.sh times out.
set -eu
W=/srv/workspaces/rustodon-remote-browser-8b2f49e-r2-alice
P=remote-browser-8b2f49e-r2
cd "$W"
test "$(cat evidence/application-revision.txt)" = 8b2f49e42a7cddd75b9d2dce4be24cfcd388099c
if podman container exists "$P-browser-alice"; then
    timeout 45 podman exec "$P-browser-alice" sh /harness/tools/remote-browser/browser close > evidence/browser-close.txt 2>&1 || true
fi
if test -f run-fixture/source.jsonl; then cp run-fixture/source.jsonl evidence/source.jsonl; fi
for n in browser forward source tls pull worker web pg migrate build; do
    if podman container exists "$P-$n-alice"; then
        podman logs "$P-$n-alice" > "evidence/$n.log" 2>&1 || true
        podman rm -f -v "$P-$n-alice" >> evidence/cleanup.txt
    fi
done
if podman network exists "$P-alice"; then podman network rm "$P-alice" >> evidence/cleanup.txt; fi
rm -rf run-fixture media target
rm -f evidence/rustodon
printf '%s\n' 'Removed task PG volume, network, media, certs, keys, env, sessions and build output.' >> evidence/cleanup.txt
podman ps -a --format '{{.Names}}' | grep "^$P-" > evidence/remaining-task-containers.txt || test $? = 1
test ! -s evidence/remaining-task-containers.txt
