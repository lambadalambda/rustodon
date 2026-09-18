# Focused remote rich-media browser acceptance

Task-only helpers for application `8b2f49e42a7cddd75b9d2dce4be24cfcd388099c`.
Not the full `mise run browser-integration` lane, a production launcher, or a peer
fixture. Owner: `meta/issues/accept-remote-rich-media-browser.md`.

## Exact launch and cleanup

The fixed `r2` namespace deliberately refuses existing containers/network and
fixture data. Run from the repository on the Mac, with the NAS SSH agent unlocked:

```sh
REV=8b2f49e42a7cddd75b9d2dce4be24cfcd388099c
W=/srv/workspaces/rustodon-remote-browser-8b2f49e-r2-alice
git archive --format=tar "$REV" | ssh root@podman-worker.local \
  "test ! -e '$W' && mkdir -p '$W/source' '$W/harness' '$W/evidence' && tar -xf - -C '$W/source' && printf '%s\\n' '$REV' > '$W/evidence/application-revision.txt'"
COPYFILE_DISABLE=1 tar -cf - tools/remote-browser tools/tests/remote-browser-test.py | \
  ssh root@podman-worker.local "tar -xf - -C '$W/harness'"
tools/mastodon-fixture verify-source target/mastodon-v4.6.5 | \
  ssh root@podman-worker.local "cat > '$W/evidence/reference-source.txt'"
ssh root@podman-worker.local \
  "cd '$W' && timeout -k 20 1500 sh harness/tools/remote-browser/run.sh > evidence/run.log 2>&1"
```

Check each command's exit status; never continue after a failed sync or source
verification. Sync only the tracked archive and explicit harness files, not `.git`,
instance environments, build output or unrelated untracked files. Keep the verified
Mastodon reference checkout read-only. The existing dependency cache is reused
in-place; compilation output is fresh and task-owned.

`run.sh` executes the entire bounded recipe: debug/test-support build in pinned
media-tools `7203e0222e2b`; PG14 restore/restricted grants; web and ingress worker;
task CA/leaf and signing key; controlled TLS media source; canonical browser TLS
forwarder; existing browser image `d6337b96fb60`; CA-verified HTTP readiness;
CLI skill loading/login; Pull worker; and uninterrupted `accept.sh`.

An EXIT trap runs `cleanup.sh` on success or failure. If the outer timeout kills
the shell, explicitly run this same narrow cleanup (never engine-wide prune):

```sh
ssh root@podman-worker.local \
  "cd '$W' && timeout -k 10 180 sh harness/tools/remote-browser/cleanup.sh"
```

It removes only the fixed task containers and PG volume/network, media root,
generated certs/keys/env, browser session containers, and task build output/binary.
Source, harness and sanitized evidence remain. Check `cleanup.txt` and the empty
`remaining-task-containers.txt`. Before exporting evidence, scan for credentials,
then create SHA256SUMS; never export raw CLI network headers/bodies/session data.

## What the controller proves

- Real RSA-signed inbox Create/Note import. Pull is stopped until the imported
  attachment has a zero-attempt, unleased fetch-media job and no cache filename.
  Owner SQL sets up identities/relationships and asserts state; it never installs
  cached media. Baseline Alice/Bob mute and exclusive-list restrictions are removed
  for ordinary home-stream eligibility, not to test relationship policy.
- Actual frontend WebSocket pending `update` then real worker `status.update`,
  observed without synthesizing frames or navigating/reloading for convergence.
  The debug/test-support origin map uses task TLS on loopback; no production fence
  changes. Media bytes and outbox transitions are installed by the real worker.
- MP4 decoded PNG poster before Play; local video currentTime and decoded frames
  advance. Audio has no preview/small, uses its decoded account-avatar fallback,
  and plays. AVIF renders decoded JPEG. Dedicated status reloads repeat all three.
- Reload validation uses the **actual frontend XHR response** from the new document,
  not a saved live attachment or diagnostic refetch. Audio explicitly rechecks
  `preview_url == null` and absence of `meta.small`.
- Each live stage and each navigation/reload has its own network recording interval.
  Records are saved and appended to an aggregate before the CLI recorder is cleared.
  Each dedicated reload starts after a boundary and must independently show a
  successful local media request and no origin-host network/DOM hotlinks. Previous
  live requests cannot satisfy reload positives. Aggregate zero-origin evidence
  survives every reset. Only allowlisted fields are retained; headers, query strings,
  fragments, bodies and tokens are excluded.

## Bounds and evidence boundary

Heavy work is serial. Build: 4 CPU/6 GiB/512 PIDs/900 seconds. PG: 1 CPU/512 MiB;
web/each worker: 2 CPU/1 GiB; browser: 2 CPU/2 GiB; source/TLS/forward:
1 CPU/128–256 MiB. Runtime containers have explicit PID and wall-time bounds,
read-only roots where applicable and no published host ports. Only the loopback
443 forwarder adds `NET_BIND_SERVICE`. Controller: 600 seconds; total: 1500 seconds.

Offline regressions: `PYTHONDONTWRITEBYTECODE=1 python3 tools/tests/remote-browser-test.py`.
The fresh final run passed uninterrupted. An earlier launcher attempt failed before
the controller on first navigation; the one permitted harness correction added
end-to-end TLS readiness. Its evidence is retained separately as
`startup-attempt-evidence/`; it is not part of the successful gate.

Actual evidence remains uncommitted under the task `evidence/`, mirrored locally
at `target/remote-browser-8b2f49e-r2-evidence/`. Private browser coverage and the full
source/HTTP/browser/peer matrix are not claimed. Existing private-byte HTTP evidence
remains separate. Harness changes are uncommitted pending parent review 2.
