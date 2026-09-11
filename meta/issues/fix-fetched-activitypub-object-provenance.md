# Fix fetched ActivityPub object provenance

## Summary

Prevent fetched Create and nested Announce wrappers from fabricating content attributed to another remote author.

## Requirements

- Bind embedded object authority to authenticated fetch provenance or independently dereference the canonical object.
- Preserve legitimate boosts whose announcer differs from the original author.

## Acceptance Criteria

- Adversarial cross-origin Create and nested Announce regression tests fail before the fix and pass afterward.
- No forged status, mention, notification, or boost is committed; legitimate remote boosts remain functional.

## Notes

- Findings R01 in [the essential feature parity review](../essential-feature-parity-review.md), baseline `1861d33`.
- Review-only discovery: no implementation fix is included in the review commit. End-to-end scenarios remain to be executed unless the report explicitly records a parser reproduction.

## Implementation and verification (R01 only)

Fetched Create and nested Announce wrappers now bind their claimed actor to the requested transport origin (scheme, host, effective port) before any embedded-object write. Nested Announce reuses an embedded Note only for an exact-author self-boost; foreign authors are resolved through the canonical object URI. Existing redirect, Note ownership, domain policy, and nesting limits remain unchanged.

### Reference and environment

- Read-only pinned Mastodon source: `/home/lain/repos/rustodon/target/mastodon-v4.6.5` on secunda, verified revision `1440d55b139e39ec722c2a3db7f60b66cd889048`. The AGENTS path `/workspace/rustodon/target/mastodon-v4.6.5` is absent locally. Upstream `FetchRemoteStatusService#trustworthy_attribution?` binds fetched activity attribution to its host; `Activity#status_from_object` reuses embedded Notes only for self-boosts, otherwise fetching the original. Rustodon's new wrapper check also enforces scheme/port equality.
- Every build, test, rustfmt, and Clippy execution ran on `lain@secunda.local`, in `/home/lain/rustodon-parity/provenance`, with Rust 1.97.1 and `CARGO_BUILD_JOBS=4`. The full worker harness used rootless Podman and its unique disposable restored-fixture resources. No local compilation, production access, live credentials, upstream source fetch, or edits to the user's remote checkout/configs occurred.
- Worktree sync command (no deletion):

  ```sh
  rsync -az --exclude='/.git' --exclude='/target/' \
    --exclude='/.local-instance/' --exclude='/.local-instance-backups/' \
    --exclude='/.env*' ./ lain@secunda.local:/home/lain/rustodon-parity/provenance/
  ```

### RED → GREEN

Commands below ran through `ssh lain@secunda.local 'bash -lc ...'`, after `cd /home/lain/rustodon-parity/provenance && export CARGO_BUILD_JOBS=4`.

1. Before the production fix, `tools/mastodon-fixture worker-test` ran the four new restored-worker regressions: **49 passed, 4 failed**, exit 101. Initial test scaffolding needed a `failed_at` → `dead_at` column correction before obtaining this behavioral RED.
2. `cargo test --locked --lib worker::tests::fetched_wrappers_reject_cross_origin_actor_provenance`: **1 failed**, exit 101, on an attacker-origin Create claiming the victim actor. Running `cargo test --locked --lib worker::tests::` while production was still unchanged also failed the cross-author embedded-target assertion (18 passed, 2 failed).
3. After the fix, test refinement removed implicit personal-inbox mentions, used the fixture's actual ID-scheme actor URI for the explicit forged mention, and used canonical `410 Gone` (the existing worker treats 404 as retryable). The final four fixtures were rerun against unmodified baseline `src/worker.rs` using `git show HEAD:src/worker.rs` copied into the isolated remote mirror, then the fixed source was restored. `tools/mastodon-fixture worker-test`: **49 passed, 4 failed**, exit 101, 44.49 seconds. Final RED log: `/home/lain/rustodon-parity/provenance-r01-red-final.log`.

   - Forged Create committed a victim Note, outer boost, explicit local mention, and notification: `(statuses, mentions, notifications) = (50, 9, 68)` versus baseline `(48, 8, 67)`.
   - Foreign embedded nested Announce committed the victim Note and both boosts: `(51, 9, 68)` versus `(48, 8, 67)`.
   - Spoofed nested actor committed a victim Note/mention/notification before the later boost validation failed: `(49, 9, 68)` versus `(48, 8, 67)`.
   - The positive cross-author case consumed the forged embedding rather than fetching canonical content, also committing the forged mention/notification.
4. Final fixed-source commands and results:

   | Command | Result |
   | --- | --- |
   | `cargo fmt --all` | Passed; only changed Rust files retrieved, never remote `target/` |
   | `cargo fmt --all -- --check` | Passed |
   | `cargo test --locked --lib worker::tests::` | 20 passed |
   | `tools/mastodon-fixture worker-test` | 53 passed, 0 failed; 42.10 seconds, plus worker process lifecycle checks |
   | `cargo test --locked --all-targets --all-features` | 407 passed, 0 failed, 129 ignored across 22 test binaries |
   | `cargo clippy --locked --all-targets --all-features -- -D warnings` | Passed |

   Final logs are `/home/lain/rustodon-parity/provenance-r01-green-final.log`, `/home/lain/rustodon-parity/provenance-r01-tests.log`, and `/home/lain/rustodon-parity/provenance-r01-clippy.log`.

The restored-worker regressions execute ingress → durable resolution → HTTP fixture fetch → writer → Core notification/distribution work. Negative cases assert unchanged status/mention/notification totals, no target or boost records, permanent failure, and the expected authoritative-fetch count. The positive case proves canonical content and original ownership, both boost owners, and both boosts' reference to the original status. Existing fetched-Note/embedded-self-boost integration remains green. New fixtures omit `summary` to isolate R01 from R04.

### Review and limits

- Independent read-only subagent review `442da291-fd6b-4263-a80c-53027b26bf3c` approved the exact production/test diff for correctness, security, and architecture/DRY/compactness before commit. It did not independently rerun commands.
- Non-blocking test-harness limitation: unexpected setup/worker errors before the final assertions can bypass scenario cleanup and cause follow-on duplicate fixture IDs. Final regression assertions run after cleanup; fresh disposable harness runs passed. No broader cleanup refactor is included.
- This is restored-fixture HTTP endpoint testing, not a live TLS peer or Rails differential execution. Other ignored integration suites were not run; the full worker suite was explicitly run. Other review findings are untouched.
- Acceptance criteria are satisfied; issue-index/archive updates are left to the parent as requested.
