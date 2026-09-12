# Build and test on Secunda

The default build/test host for essential-parity work is
**`lain@secunda.local`**, not the local coding machine. The user-authorized NAS
exception and its exact peer workspace contract are documented in
[Testing on NAS](testing-on-nas.md). During the audit implementation the parent
alone runs all workloads sequentially on NAS; child worktrees only edit source.
Source editing and Git commits may otherwise remain local. Queue-performance
review observations R17/R18 are deferred.

## Host and isolation

- Linux x86-64, Rust/Cargo 1.97.1, rustfmt, and Clippy are installed.
- Rootless Podman 6.1.1 uses netavark. Podman, slirp4netns, fuse-overlayfs, and
  their dependencies were installed with the user's permission.
- Use a separate `/home/lain/rustodon-parity/<task>/` directory for each worktree.
  Do not overwrite the existing `/home/lain/repos/rustodon` checkout, its
  untracked `tracked-configs/`, or another task's workspace.
- Do not run broad container/image/volume pruning. The fixture harness creates
  PID-specific resources and cleans up its own resources.
- SSH's remote login shell is Fish; invoke `bash` explicitly for shell scripts.
  If the SSH agent fails to sign, ask for an unlock instead of changing keys.

## Source synchronization

For a fresh task directory, synchronize only tracked files (including new test
files explicitly added to Git before the red run):

```sh
# Local coding machine, from the intended Git worktree; choose a unique TASK.
TASK=example-fix
ssh lain@secunda.local "mkdir -p /home/lain/rustodon-parity/$TASK"
git ls-files -z | rsync -az --from0 --files-from=- ./ \
  "lain@secunda.local:/home/lain/rustodon-parity/$TASK/"
```

This deliberately does not copy `.git`, ignored build output, instance
environments, secrets, backups, or unrelated untracked files. Do not add
`rsync --delete`; use a fresh task directory when deleted source files would
otherwise leave stale remote copies. Keep new test fixtures explicitly tracked
so red/green runs exercise them. Retrieve only intentionally formatted source
files when running `cargo fmt` remotely; never sync remote `target/` back.

## Existing pinned Mastodon source

The AGENTS.md reference `/workspace/rustodon/target/mastodon-v4.6.5` is not present
on the local coding machine. Secunda already has the exact reference at:

```text
/home/lain/repos/rustodon/target/mastodon-v4.6.5
1440d55b139e39ec722c2a3db7f60b66cd889048
```

Read it in place; do not clone/fetch another copy or modify it. For gates that
need upstream source, verify the revision and link it into the isolated task:

```sh
# Run on Secunda in the isolated task directory.
SOURCE=/home/lain/repos/rustodon/target/mastodon-v4.6.5
test "$(git -C "$SOURCE" rev-parse HEAD)" = \
  1440d55b139e39ec722c2a3db7f60b66cd889048
mkdir -p target
ln -s "$SOURCE" target/mastodon-v4.6.5  # once, only when the destination is absent
```

Clean-checkout unit/Clippy verification must run **without** that source link.
Do not delete the shared source to simulate its absence. The dedicated
`tools/pinned-source-contracts` command verifies the referenced checkout before
executing its ignored upstream-contract tests.

## Running gates

A remote command pattern (adjust the task path):

```sh
ssh lain@secunda.local bash -s <<'REMOTE'
set -eu
cd /home/lain/rustodon-parity/example-fix
export CARGO_BUILD_JOBS=4
cargo fmt --all --check
cargo test --locked --all-targets --all-features
cargo clippy --locked --all-targets --all-features -- -D warnings
tools/mastodon-fixture verify
REMOTE
```

Run appropriate integration gates on Secunda too, with the pinned source link
when required:

```sh
tools/mastodon-fixture schema-read-test
tools/mastodon-fixture worker-test
tools/mastodon-fixture operational-schema-test
tools/mastodon-fixture differential-test <case-name>
tools/pinned-source-contracts
```

These are existing gates, not a claim that every command above has passed for
every worktree. Check the owning issue for exact red/green commands and results.
The complete release gate also includes dependency policy, startup/preflight,
browser, and cutover checks from `mise.toml`; install/verify any missing pinned
tools on Secunda rather than silently skipping them or running locally.

## Peer-test acceptance

Mastodon and Pleroma peer tests must use isolated accounts, databases, origins,
and media—not the live tunnel or `lain.com`. Preserve the production SSRF and
signature boundaries; any test-only endpoint routing must be explicit and
unavailable in ordinary release builds. Assert received objects, actor IDs,
audiences, access denial, notifications, and lifecycle convergence. HTTP 2xx
from an inbox only establishes acceptance/queuing, not successful ingestion.

Peer harness implementation and actual direction/activity evidence are tracked
in `meta/issues/run-essential-parity-gates-on-secunda.md`; this document does not
claim that the full bidirectional peer matrix is already implemented or passing.
