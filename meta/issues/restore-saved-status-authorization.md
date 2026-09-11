# Restore saved-status authorization

## Summary

Recheck current root-status authorization when returning bookmarks and favourites.

## Requirements

- Do not treat association ownership as continuing access to a private status.
- Preserve association pagination semantics when filtering unauthorized statuses.

## Acceptance Criteria

- After follower removal and a subsequent author edit, neither saved endpoint exposes the private status or its new content.
- Mention-granted access and ordinary authorized saved-status reads remain correct.

## Notes

- Findings R02 in [the essential feature parity review](../essential-feature-parity-review.md), baseline `1861d33`.
- Originally a review-only discovery; implementation and executed evidence follow.

## R02 implementation and evidence

- Saved roots now use the same current-viewer authorization loader as status-show.
  Association ownership no longer grants continuing private-status access.
- Page boundaries and fullness come from the selected associations, before filtering.
  A full but entirely hidden page retains next/previous links; visible statuses do
  not replace association cursors.
- `tests/saved_status_authorization.rs` exercises the production HTTP router against
  a disposable restored fixture: a follower bookmarks and favourites five statuses,
  the author removes that follower and edits two private statuses, status-show denies
  access, and both saved endpoints exclude the statuses and new content. Public,
  private-mention, and direct-mention reads survive. It also checks association order
  differing from status order, hidden boundaries, fully filtered pages, exhausted
  pages, and `max_id`, `min_id`, and `since_id` navigation.
- `tools/mastodon-fixture schema-read-test` runs the new test in its own restored
  database, separately from the existing schema suite. The lifecycle intentionally
  mutates only disposable fixture data.

All builds, tests, Clippy, and formatting ran on `lain@secunda.local`, in
`/home/lain/rustodon-parity/saved-auth`, using Rust 1.97.1 and rootless Podman.
No local Rust execution or live instance credentials were used. Source was synced
with this command (no deletion or remote target sync-back):

```sh
rsync -az --exclude='/.git' --exclude='/target/' \
  --exclude='/.local-instance/' --exclude='/.local-instance-backups/' \
  --exclude='/.env*' ./ lain@secunda.local:/home/lain/rustodon-parity/saved-auth/
```

The exact RED/GREEN command, before and after the production fix respectively:

```sh
ssh lain@secunda.local 'bash -lc '\''cd /home/lain/rustodon-parity/saved-auth && CARGO_BUILD_JOBS=4 tools/mastodon-fixture schema-read-test'\'''
```

- **RED:** exit 101; 37 existing schema tests passed, new lifecycle test failed.
  Its combined assertion showed both bookmarks and favourites returning five IDs
  and the new-private-content marker (`true`), instead of three authorized IDs and
  no marker (`false`). Status-show 404 assertions had already passed. Remote log:
  `/tmp/r02-red.log`.
- **GREEN:** exit 0; 37 existing schema tests and the new lifecycle test passed,
  including all pagination assertions. Repeated after remote formatting:
  `/tmp/r02-green-final.log` (37 + 1 passed).
- Before the behavioral RED, test setup required correcting a migration call,
  separating restored databases to avoid existing schema-suite catalog changes,
  and using the positive-ID local follower fixture because stream events reject
  negative account IDs. Those setup failures were not counted as behavioral RED.

Additional remote commands (same SSH/bash prefix and working directory):

```sh
cargo fmt --all
cargo fmt --all -- --check
CARGO_BUILD_JOBS=4 cargo clippy --locked --all-targets --all-features -- -D warnings
CARGO_BUILD_JOBS=4 cargo test --locked --all-targets --all-features
```

All passed. The ordinary test run reported **406 passed, 0 failed, 126 ignored**
across 23 binaries; ignored fixture tests are not implied to have run. The first
all-target test invocation exceeded the local SSH tool's 120-second wait; the
explicit rerun completed with exit 0 (`/tmp/r02-tests-final.log`). Clippy log:
`/tmp/r02-clippy.log`. Only the changed formatted Rust files were retrieved.

The read-only pinned source was inspected remotely at
`/home/lain/repos/rustodon/target/mastodon-v4.6.5`, verified revision
`1440d55b139e39ec722c2a3db7f60b66cd889048` (symlinked in the mirror's target).
The AGENTS path `/workspace/rustodon/target/mastodon-v4.6.5` was absent locally.
Upstream bookmarks/favourites controllers paginate association results and base
continuation on association count; no Mastodon source was fetched or modified.
This was a Rustodon HTTP regression, not a live Mastodon differential run.

Independent read-only review found no actionable R02 correctness or
architecture/DRY/compactness issues. Review was source-only, without execution or
Git/SSH tools. It noted a separate shared-loader concurrency concern: authorization
and projection queries do not share a snapshot. Concurrent revocation/edit races
were not tested or redesigned in this sequential lifecycle fix.

Implementation is complete; issue indexes and archival status remain unchanged as
requested. No other review findings were fixed.
