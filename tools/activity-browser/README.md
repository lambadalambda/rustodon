# Bounded instance-activity browser acceptance

Owner: [acceptance subissue](../../meta/issues/accept-instance-activity-browser.md).
Application **10ddf66caf1fe8714f7c41ac4a1a0b8ceaf7fe34**, default-feature debug
main binary (no `test-support`), SHA-256
`571f4b8c6d33ab4dad83eb631a7c40ab44ef7c5713f1b5e2441228519d239e7c`.

This fixed adaptation reuses `tools/remote-browser` setup, TLS, Chromium image,
CLI wrapper, shared `ui.py` helpers and cleanup. No new framework, application
changes, clock overrides, test endpoints, production access or deployment.
`observe.js` passively records allowlisted completed instance XHR fields; it does
not replace responses or write rendered counts. The controller waits for the
actual frontend XHR and rendered count, including limited mode and fresh reloads.

## Reproduction

Load `agent-browser` / `nas-podman` skills and `agent-browser skills get core`.
Use an unlocked SSH agent; keep NAS operations sequential. The fixed workspace
must not exist on initial launch (retain existing evidence; do not overwrite it).
Run each command with failure checking and shell pipeline failure propagation:

```bash
set -euo pipefail
REV=10ddf66caf1fe8714f7c41ac4a1a0b8ceaf7fe34
W=/srv/workspaces/rustodon-activity-browser-10ddf66-alice
tools/mastodon-fixture verify-source target/mastodon-v4.6.5
PYTHONDONTWRITEBYTECODE=1 python3 tools/tests/activity-browser-test.py
PYTHONDONTWRITEBYTECODE=1 python3 tools/tests/remote-browser-test.py
git archive --format=tar "$REV" | ssh root@podman-worker.local \
  "test ! -e '$W' && mkdir -p '$W/source' '$W/harness' '$W/evidence' '$W/reference' && tar -xf - -C '$W/source' && printf '%s\\n' '$REV' > '$W/evidence/application-revision.txt'"
COPYFILE_DISABLE=1 tar -cf - $(git ls-files tools/remote-browser) \
  tools/activity-browser/prepare.py tools/activity-browser/ui.py \
  tools/activity-browser/observe.js tools/activity-browser/stages.sh \
  tools/tests/activity-browser-test.py | ssh root@podman-worker.local \
  "tar -xf - -C '$W/harness' && cd '$W' && python3 harness/tools/activity-browser/prepare.py harness harness/tools/activity-runtime"
# Only exact verified reference files consumed by the focused existing contract.
# No .git, instance environments, build output, credentials or unrelated files.
git -C target/mastodon-v4.6.5 archive 1440d55b139e39ec722c2a3db7f60b66cd889048 \
  app/lib/activity_tracker.rb app/models/user.rb \
  app/controllers/concerns/user_tracking_concern.rb \
  app/controllers/api/base_controller.rb \
  app/controllers/api/v1/accounts/credentials_controller.rb \
  app/presenters/instance_presenter.rb app/serializers/node_info/serializer.rb \
  Gemfile.lock | ssh root@podman-worker.local "tar -xf - -C '$W/reference'"
tools/mastodon-fixture verify-source target/mastodon-v4.6.5 | \
  ssh root@podman-worker.local "cat > '$W/evidence/reference-source.txt'"
ssh root@podman-worker.local \
  "cd '$W' && timeout -k 20 1500 sh harness/tools/activity-runtime/run.sh > evidence/run.log 2>&1"
```

The generated scripts contain the exact container/build/setup/controller commands.
Retain their hashes with the executed application manifest, reference hashes and
image identities. If the outer timeout interrupts the EXIT trap, run only:

```sh
ssh root@podman-worker.local \
  "cd '$W' && timeout -k 10 180 sh harness/tools/activity-runtime/cleanup.sh"
```

## Executed final gate

Fresh final uninterrupted run passed. Earlier first pass retained separately on
NAS; final strengthens limited-mode render waiting and adds focused Rust checks.

| Stage | Public sidebar | v2 / initial monthly | NodeInfo month / half-year |
| --- | --- | --- | --- |
| Migration 6, empty history | 0 | 0 / 0 | 0 / 0 |
| Real confirmed Alice login today | Signed-in home (no public sidebar) | 0 / 0 | 0 / 0 |
| Simulated historical bucket, ordinary | 1 | 1 / 1 | 1 / 1 |
| Same fixture, limited federation | 0 | 0 / 0 | 1 / 1 |

All public stages repeat after actual reload. Historical and limited screenshots
show the real pinned frontend's `active users` sidebar, not synthetic markup.
Browser `fetch` obtains real v2 and NodeInfo responses; initial metadata is read
from the actual initial-state script. Each public document independently observes
the frontend's v2 XHR before assertions; no delayed-instance fixed sleep.

Setup only clears the fixture Alice's sign-in timestamp so native password +
recovery-code login is due. The real restricted writer/auth recording helper
creates one confirmed, approved Alice membership **today**. Owner SQL asserts
that membership, then moves that bucket and member date to yesterday, preserving
identity/expiry, without inserting activity. The web process restarts to avoid
waiting for cache TTL. This is **seeded simulated history**, not historical
backfill, an observed UTC rollover or production-clock manipulation. Existing
UTC/cache library tests and prior main-HTTP coverage are separate evidence.

## Bounds and actual checks

- Immutable tools `7203e0222e2b`, browser `d6337b96fb60`, PG14.23
  `1a6c2409ab71`; all full IDs in evidence. Pinned frontend pack integrity passed.
- Serial build/check container: 4 CPU / 6 GiB / 512 PIDs / 870s, outer 900s.
  PG: 1 CPU / 512 MiB / 128 PIDs / 5400s; web: 2 CPU / 1 GiB / 256 PIDs /
  5000s (limited replacement 900s). Browser: 2 CPU / 2 GiB / 256 PIDs / 1200s;
  TLS: 1 CPU / 256 MiB; forward: 1 CPU / 128 MiB, both 256 PIDs / 1200s.
  Controller 600s, individual stages 150s, total launcher 1500s. Internal
  task-only network, no published ports; only loopback TLS forward adds bind cap.
- PG migration ledger **1–6**; restricted roles have no superuser/creator/bypass
  flags or memberships. `pg_stat_activity` proves runtime and writer attachments;
  full table grants retained and runtime-no-INSERT/writer-INSERT asserted.
- Focused default-feature library `activity::`: **3 passed, 7 ignored**.
  Existing pinned daily-activity source contract: **1 passed**, consuming an
  immutable subset archived from the locally verified clean exact reference,
  not a remote Git checkout and **not the full pinned-source lane**.
- New adaptation regressions **2 passed**; unchanged shared media regressions
  **8 passed**. Initial missing-module TDD red retained. The production behavior
  is acceptance-tested, not reimplemented. Diff check passed.
- Full source/DB/browser/worker/peer/differential matrix, main HTTP rerun,
  midnight waiting, real historical backfill and release claims are excluded.

## Evidence, provenance and cleanup

Local: `target/activity-browser-evidence/evidence/`; NAS: `$W/evidence/`.
Key files: `historical{,-reload}.png/json`, `limited{,-reload}.png/json`,
`baseline{,-reload}.json`, `today-after-real-login.json`, `today-membership.txt`,
`simulated-history.txt`, `grant-attach-proof.txt`, `role-proof.txt`,
`baseline.sql.txt`, `build.log`, `application-manifest*.{json,txt}`,
`harness-SHA256SUMS`, `reference-SHA256SUMS`, `image-identities.txt`, `SHA256SUMS`.

All **6,480** tracked app files/symlinks were compared with immutable Git archive
contents. An initial manifest attempt incorrectly included the locally edited
issue index; it failed only on that documentation file. The corrected Git-object
manifest passed against the executed archive; application source was never
modified. A preparation drift guard also rejected an ambiguous mount replacement
before final generation; corrected to a unique context and regressions passed.

Task browser session, containers, PG volume/network, media, certificates, keys,
environment and build output/binary removed. Browser/forward required bounded
SIGKILL during teardown after their stop grace; no resources were retained and
this happened after successful acceptance. No raw headers, tokens, session files
or private keys exported; credential-pattern scan passed (the grant inventory
naturally names the OAuth access-token table, not token values). Reference/source,
harness and sanitized evidence only are retained. No broad resource pruning.

Initial independent delegation was rejected at the session nesting limit. Parent
review **980993** subsequently found no high-severity blocker and approved focused
closure from the combined reviewed evidence. The activity parent and its three
subissues are archived; the wider combined acceptance sweep remains pending.
Returning tracking acceptance covers interactive HTML/credentials/settings/session
hooks, not all API/lifecycle tracking. Exact DISTINCT counting intentionally
replaces approximate reference HLL, without claiming full tracking parity.

Nonblocking medium harness follow-ups are recorded in the acceptance issue:
cleanup/status handling can mask cleanup failure (this run has separate successful
`final-cleanup-proof.txt`), and negative observer unit tests are missing. These are
recorded for later focused work, not expanded or silently fixed in this closure.
