# Testing on the authorized NAS

This is the user's explicit exception to the default
[Secunda policy](testing-on-secunda.md), not permission to test on the local
coding machine or on a production instance. During the test-audit implementation
**the parent runs every build, test, formatter, lint, and container workload
sequentially**. Child agents prepare source and commands only. Never add this NAS
as a self-hosted CI runner or run the heavy lanes against production.

## Host, synchronization, and resource isolation

- Worker hostname: **`podman-worker`** (`podman-worker.local` for access).
  Use task-specific workspaces beneath `/srv/workspaces/`, not instance checkouts.
- Synchronize only the intended tracked source plus an explicit list of new
  regression/fixture files. `git ls-files` omits unstaged new files. Do not copy
  `.git`, `target/`, instance environment files, credentials, backups, or unrelated
  untracked files. No `rsync --delete`; use a fresh directory for removed files.
- Use the parent's existing authorized tooling container and Podman context; do
  not switch socket privileges or change the default connection. Secunda/hosted
  CI use rootless Podman. The authorized NAS service is a separate environment;
  do not claim it is rootless or silently replace it with a privileged service.
- Preserve container names, run markers, separate DB roles/databases, synthetic
  identities/origins, and media roots. No live tunnel, production env, or `lain.com`.
- Keep `CARGO_BUILD_JOBS=2` and Mise task concurrency at one (`MISE_JOBS=1` or
  `mise run -j 1 ...`). Run one gate at a time. Bound the tooling container's
  resources and each workload's wall time; the NAS has 12 GiB RAM total, so leave
  host headroom. Do not launch concurrent Rust builds, fixture matrices, or peers.
- Retain per-command exit status and logs in the task workspace. Never use a
  successful `tee` exit as evidence that the preceding command passed; use Bash
  `set -o pipefail`. No broad image/container/volume pruning. Clean only resources
  created by the current run. Ask for an SSH-agent unlock instead of workarounds.

## Task-owned API socket for bridge-backed gates

The global rootful API service's missing user-bus environment prevents its DNS
helper from starting on this NAS. Do not restart/reconfigure that shared service.
From an active **root SSH login**, use `tools/nas-fixture-session -- COMMAND...`
in the physical `/srv/workspaces/rustodon-<task>/source` directory. It requires
the existing `/run/user/0` bus and creates a private, task-owned API socket using
the same engine. No root linger or global configuration change is needed.

The command must explicitly consume `RUSTODON_NAS_SOCKET`: bind
`"$RUSTODON_NAS_SOCKET:/run/podman/podman.sock"` into the existing tooling image,
whose Podman executable wrapper selects that endpoint. Merely exporting this
variable does not redirect arbitrary Podman clients. Keep the same absolute
workspace bind, host networking, cached tool image and resource limits above.

The helper supervises the command's own process group, forwards cancellation,
allows bounded fixture cleanup with the API still available, then stops only
its API process and removes its socket. It retains `../ops/nas-api.*/service.log`.
Fixture containers/volumes/networks remain the fixture harness's responsibility;
keep its wall-time bounds and cleanup verification. Do not run concurrent gates.

## Source contracts versus image-only fixtures

The source-contract oracle remains the read-only Mastodon **4.6.5** checkout at
`/workspace/rustodon/target/mastodon-v4.6.5`, or the existing Secunda counterpart
`/home/lain/repos/rustodon/target/mastodon-v4.6.5`, revision
`1440d55b139e39ec722c2a3db7f60b66cd889048`. Do not fetch a replacement when that
checkout exists; do not alter it or substitute a different version.

`mise run pinned-source-contracts` is a **separate source-dependent lane**. Verify
and expose the authorized read-only checkout at the task's
`target/mastodon-v4.6.5` only for source-dependent commands. If it is not available
on NAS, report the lane blocked; image-only success is not source-contract proof.
Differential media writes and cutover/browser now reuse the verified vendored
JPEG instead of depending on an ignored source checkout merely for an image.
Fresh hosted CI may still obtain the pinned source for its separate oracle lane;
this is not an instruction to fetch another NAS reference checkout.

Workers use the parent's vendored `fixtures/worker-media/mastodon-v4.6.5` corpus
and `tools/verify-worker-media`; a whole source checkout is not their prerequisite.
The peer NAS lane is **cached pinned images only**, with no source checkout
requirement. It must verify immutable image digest/platform and fixture metadata,
fail on missing or mismatched cached images, and never pull/build upstream as a
fallback. Keep the source-dependent lane separate rather than disabling its guard.

## Configured gate map (not execution evidence)

| Lane | Commands | Trigger / prerequisites |
| --- | --- | --- |
| Bounded ordinary checks | `mise run -j 1 check` | Required CI: formatting, Clippy, ordinary default/debug-all-feature/release-all-feature tests, dependency policy, static fixtures, vendored worker assets, offline harnesses. Ignored integration tests are **not** executed here. |
| Fast harnesses only | `tools/check-harnesses` | Python 3.11+, POSIX shell/Bash. Runs offline `tools/tests/*-test` and `*-test.py`, Pleroma build guards, and TLS proxy audit units; not browser/peer acceptance. |
| Focused HTTP/schema | `mise run mastodon-schema-integration` | Required CI; parent's schema aggregate includes the permanent HTTP selectors, including media proxy. Restored disposable DB/roles. |
| Operational/startup/preflight | `mise run operational-schema-integration`, `mise run startup-integration`, `mise run preflight-integration` | Separate required matrix entries; run sequentially on NAS. |
| Workers | `mise run worker-media-verify`, then `mise run worker-integration` | Verified vendored media; parent fixture runner serializes worker tests. |
| Required differential | `mise run differential-ci` | Nine distinct cases, ten invocations; fresh fixture clones per invocation. Bearer/status authorization, signed fetch, browser recovery fences and reauthentication limits, REST serializers/protocol, federation discovery, and both actor-media-root modes. |
| Broader differential | `mise run differential-full` | Full fixture suite plus relative actor-media-root mode; verified vendored media prerequisite. Weekly/manual hosted CI after required gates, never a `check` dependency. |
| Cutover/browser | `mise run cutover-integration`, `mise run browser-integration` | Weekly/manual hosted CI, sequential matrix, 180-minute job deadlines. Disposable fixtures only; verified vendored media prerequisite. Browser CLI `agent-browser` 0.31.1, Node 24.15.0 and installed Chromium/system dependencies; no automatic NAS installation. |
| Pinned source | `mise run pinned-source-contracts` | Required separate hosted job; verified read-only checkout on authorized hosts. |
| Real peers | `mise run peer-public`, `peer-privacy`, `peer-notes`, `peer-profile`, `peer-interactions` | Manual authorized-host commands only; see exact NAS contract below. No workflow job. |

The mixed-profile selector uses test-profile opt-level3 for both discovery and
execution, preserving debug assertions and the exact animated fixture's2s connect,
10s total and5s read budgets. The full differential lane skips that case in its
ordinary batch and runs it once afterward with the scoped optimization. Other
selectors and CLI builds keep their caller profile.

The browser lane exercises fixture-authenticated shell/settings/logout plus real
Home boost/reply actions: leading and trailing settings PUTs, omission of the
client-only `saved` field, and bootstrap/rendered persistence after reload. HTTP
observation starts before navigation and rejects unexpected API failures; deliberate
marked auth/missing-resource probes remain exact. This is not coverage of every
browser form, WebSocket, or EventSource behavior. Do not use blanket
`cargo test -- --ignored` or concurrent fixtures to replace named gates.

Browser cutover uses `tools/browser-fixture-tls`: a task-owned loopback TLS relay
with an ephemeral exact-domain certificate. It passes `RUSTODON_BROWSER_CA_FILE`
and `RUSTODON_BROWSER_SPKI` only to the smoke invocation. Curl validates that CA
and hostname; Chromium trusts only the fixture leaf SPKI, not arbitrary invalid
certificates. No global trust installation or production CSRF/cookie changes.
TLS certificates are removed even when other cutover artifacts are retained.
The offline harness now requires OpenSSL as well as Python for real loopback TLS
transport, negative trust/hostname, streaming, and bounded-cleanup tests.

## Exact NAS peer execution contract

Parent owns the NAS adaptation of `tools/federation-peer-smoke`. Tasks invoke it
directly without overrides. The guarded NAS adaptation validates the actual engine in addition to the
runner hostname. Never patch guards at runtime or bypass source/image checks.

- Real worker hostname must be `podman-worker`.
- Physical work directory must be exactly
  **`/srv/workspaces/rustodon-peer-tests/source`** (not a symlink alias).
- The parent's tooling container uses **host networking** and binds that directory
  at the **same absolute path**, with its working directory set there. Bind paths
  are resolved by the NAS Podman service, not the local coding machine. The runner
  must validate the actual authorized worker, not merely a caller-supplied label.
- Use only cached pinned images; no source dependency in this peer lane. Preserve
  image/fixture verification, per-run markers, test-list nonzero guards, deadline,
  cleanup and production SSRF/signature boundaries. Do not expose test routing in
  ordinary release binaries.

Run the five commands individually, waiting and recording received-state/privacy
evidence for each before the next. A queued HTTP 2xx, synthetic worker pass, or
TLS proxy unit pass does not prove bidirectional peer convergence. Mastodon peer
results also do not certify Pleroma.

## Evidence status

Parent reported the phase-1 automation regression on NAS: **10 tests,
FAILED (failures=11, errors=6)**, retained at
`/srv/workspaces/rustodon-audit-main/logs/automation-red.log`. It exposed missing
configuration and runner wiring before production config edits.

Phase-2 offline validation passed on NAS: all 10 automation regressions and
`tools/check-harnesses` (shell mocks and Python units). Combined Rust validation
also passed 85 workers, seven MIME units, one private-media HTTP regression,
default/all-feature debug tests, release tests, formatting and strict Clippy.
The final eleven-selector schema aggregate passed, including the 14-request
media-state matrix. These are not evidence for
GitHub-hosted execution, full differential, browser/cutover, source contracts,
or real-peer convergence. The earlier operational-schema Rails reopen hit NAS bridge DNS resolution
failure, now resolved by the task-owned API session above; see
[the DNS blocker](../meta/issues/repair-nas-fixture-network-dns.md).

Keep exact execution evidence and configured-only lanes separate in
[the owning issue](../meta/issues/expand-automated-integration-gates.md).

Startup (five tests) and configuration preflight passed. All five actual Mastodon
peers subsequently passed after the private-unboost correction; see the exact
[peer outcomes](../meta/issues/adapt-and-run-peer-matrix-on-nas.md). These peer runs
precede the final browser API additions, rather than certify the final tree. Do not
share compiled Cargo targets between source roots; embedded workspace paths are
part of peer isolation.

Use a workload-side watchdog for long fixture commands, for example
`timeout --signal=TERM --kill-after=60s 1200s tools/mastodon-fixture worker-test`
**inside** the NAS tooling container. An SSH-client timeout alone can leave the
remote process alive. On timeout, inspect only that task's process/container IDs
and let fixture cleanup run while its task-owned API socket is still available;
do not prune the engine or terminate unrelated workloads. The1200-second example
is a test-workload bound, not a production queue/HTTP/SMTP timeout.
For schema coverage, bound each of the12 named selectors separately; the final16
aggregate cutoff after six successes was not an assertion failure. Preflight has
many canonical/drift checks and needs an aggregate budget appropriate to that work;
600/1200-second aggregate cutoffs did not establish a failing individual check.
The final18 diagnostic run printed fixed case labels only and passed all checks
under a3600-second workload bound, with unchanged production/check deadlines.

### Remaining-audit final evidence

Logs below are beneath `/srv/workspaces/rustodon-audit-green/logs/`:

- `final17-schema-*.log`: all12 independent selectors pass (including batch
  accounts); these precede the final test-only budget-clock/cleanup seams.
- `final18-{default,feature,release,clippy,harnesses}.log`: combined-tree ordinary
  default/debug-all-feature/release-all-feature tests, strict lint, offline checks.
  `final19` additionally checks formatting. Ignored tests are not inferred.
- `final18-operational.log`: complete operational/Rails gate, including actual
  asynchronous domain-lease cleanup and the fixed-clock regression.
- `startup-retry.log`: all5 startup cases pass. `final18-preflight.log`: canonical
  and all configuration-drift cases pass with sanitized stage-only diagnostics.
- Required differential: seven invocations in `final17-diff-*.log` pass;
  `final18-{oauth-diff,core-diff,reauth}.log` supplies the remaining three greens.
  The reauthentication case includes the deterministic per-instance fixture clock;
  earlier `required-remaining-complete.log` is separate, earlier-source evidence.
- `final19-browser.log`: authenticated actual leading/trailing PUT and reload
  persistence **plus full cutover/rollback** pass. `final19-cutover.log`: ordinary
  cutover independently passes. Intermediate HTTP CSRF422, Puma readiness, exact
  settings rollback and pending-leading-response failures remain recorded; no
  blanket TLS bypass, removed equality guard, or widened browser deadline.
- The latest worker rerun exposed fixture coordination failures after the earlier
  100/100 pass; [the focused follow-up](../meta/issues/stabilize-worker-executor-coordination-tests.md)
  remains open pending a restored combined worker result.

This is not a full `mise run check`, full differential, hosted-CI, Pleroma, or
read-only source-contract execution claim. Earlier five-peer results retain their
source/evidence boundary above.

Dependency-policy execution note: the currently provisioned NAS browser/tooling
image does not include `cargo-deny`. The new Markdown dependency resolves under
the committed lockfile and compiles/tests on NAS, but a fresh dependency-policy
run is not claimed from those checks. The configured CI policy lane remains
separate; no host-wide tool installation or policy bypass was performed.
