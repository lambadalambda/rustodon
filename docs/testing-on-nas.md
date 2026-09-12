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
Full differential media writes and cutover/browser currently also read
`attachment.jpg` under that source path. Fresh disposable hosted CI
obtains/verifies the pin for all extended lanes; this is not an instruction to
fetch another NAS reference checkout.

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
| Broader differential | `mise run differential-full` | Full fixture suite plus relative actor-media-root mode; pinned source media asset prerequisite. Weekly/manual hosted CI after required gates, never a `check` dependency. |
| Cutover/browser | `mise run cutover-integration`, `mise run browser-integration` | Weekly/manual hosted CI, sequential matrix, 180-minute job deadlines. Disposable fixtures only; cutover source asset prerequisite. Browser CLI `agent-browser` 0.31.1, Node 24.15.0 and installed Chromium/system dependencies; no automatic NAS installation. |
| Pinned source | `mise run pinned-source-contracts` | Required separate hosted job; verified read-only checkout on authorized hosts. |
| Real peers | `mise run peer-public`, `peer-privacy`, `peer-notes`, `peer-profile`, `peer-interactions` | Manual authorized-host commands only; see exact NAS contract below. No workflow job. |

The browser lane exercises the existing fixture-authenticated shell/settings/logout
smoke. Wiring it does not claim coverage of every browser form or the debounced
settings-save action. The broader lanes are bounded separately and are not called
“all tests.” Do not use blanket `cargo test -- --ignored` or concurrent fixture
runs to replace the named gates.

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
The complete ten-selector schema aggregate passed. These are not evidence for
GitHub-hosted execution, full differential, browser/cutover, source contracts,
or real-peer convergence. Operational-schema Rust cases passed, but its later
Rails reopen hit NAS bridge DNS resolution failure; see
[the DNS blocker](../meta/issues/repair-nas-fixture-network-dns.md).

Keep exact execution evidence and configured-only lanes separate in
[the owning issue](../meta/issues/expand-automated-integration-gates.md).
