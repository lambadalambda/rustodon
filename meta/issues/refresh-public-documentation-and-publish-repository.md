# Refresh public documentation and publish the repository

## Summary

Bring the README and related public-facing documentation in line with the current implementation and verified test evidence, add the supplied Rustodon image, and publish the project as a public GitHub repository.

## Requirements

- Correct stale README claims about project status, implemented functionality, quality gates, browser and peer evidence, and writable cutover requirements.
- Reconcile stale route, worker, differential, and administration summaries in the v1 acceptance matrix with the current source and retained evidence.
- Add the supplied `rustodon.png` image to the README with useful alternative text.
- Link the README to the public repository at `https://github.com/lambadalambda/rustodon`.
- Create that GitHub repository as public and push the complete `main` branch without exposing ignored or local-only files.

## Acceptance Criteria

- Public-facing counts and capability claims agree with the current source, configured gates, and `docs/testing-on-nas.md` evidence boundaries.
- README links resolve to tracked repository paths and clearly distinguish ordinary checks, hosted integration lanes, manual peer tests, and production evidence.
- The supplied image is tracked and rendered near the top of the README.
- `gh repo view lambadalambda/rustodon` reports public visibility and the pushed remote `main` points at the final local commit.
- Documentation receives an independent correctness and maintainability review.

## Notes

- Most changes are documentation/assets. The canonical repository metadata also
  feeds the public instance API's source URL, so that small runtime metadata
  change must receive normal source verification on the authorized NAS; no
  Rust/container workload runs locally.
- Do not claim deployment, final-tree peer reruns, Pleroma interoperability, full hosted-CI execution, or historical replay.

## Completion

- Refreshed the README's status, implemented surface, test-lane map, browser/peer evidence, writable-cutover distinction, and administrative overview.
- Added the supplied `rustodon.png` mascot and linked the public repository.
- Reconciled route, CLI, differential, worker, browser, peer, and v2-search evidence across the acceptance matrix and related runbooks.
- Set `Cargo.toml`'s canonical repository metadata and use it for the instance API source URL; removed an obsolete extended-CI source checkout prerequisite.
- Created `https://github.com/lambadalambda/rustodon` as a public repository and pushed only `main`. GitHub Actions were disabled before the first push because hosted workloads were not authorized by the NAS-only execution policy.

## Verification

- Independent correctness/DRY/readability review approved the final documentation and metadata changes after two follow-up passes.
- On the authorized NAS, `cargo fmt --all --check` and `cargo test --locked --all-targets` passed after the runtime repository-metadata change. Later source changes were comments only.
- `git diff --check` passed, and all relative links across the README and eight public Markdown documents resolved to tracked paths.
- A pre-publication scan covered 6,938 unique historical blobs reachable from `main`; no GitHub, AWS, Slack, Stripe, or Google token pattern or sensitive local-instance path was found. Private-key and credential-URL matches were confined to documented synthetic fixtures/tests.
- GitHub reports `visibility=public`, `default_branch=main`, Actions disabled, and only the `main` branch published. No deployment, replay, Pleroma run, or hosted-CI result is claimed.
