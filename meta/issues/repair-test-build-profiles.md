# Repair default and release test build profiles

## Summary

An ignored SMTP worker test calls a feature-only helper without a gate; positive synthetic transport tests conflict with debug-only test APIs in release builds.

## Requirements

- Gate the individual feature-dependent test, not the whole worker target.
- Match positive transport tests to debug-only capabilities; retain production/release negative-capability tests.
- Do not enable production network bypasses or silently erase unrelated tests.

## Acceptance Criteria

- Reproduce default-feature compilation failure and affected release-profile failure before fixes.
- Default and feature-enabled debug gates pass; appropriate release profile tests/checks pass and capability denial remains asserted.

## Notes

- Source: [test coverage audit](../test-coverage-audit.md).
- Implementation plan recorded on 2026-09-12; no live deployment is implied.

## Implementation and verification — 2026-09-12

- A task-owned isolated-worker workspace used an exact source transfer with three
  public symlinks and a bounded tool image at source revision `b2937cf` (Rust 1.97.1),
  four-CPU quota, 8 GiB memory, three Cargo jobs, capabilities dropped. No local workloads.
- Red: default-feature all-target compile fails on missing `with_complete_fault`;
  release `bounded_transport_fixtures_fail_closed` fails its `BodyTooLarge` assertion.
  Historical external run artifacts are not in the repository.
- Gate only the individual SMTP fault-injection test. Match remote positive
  synthetic-transport tests/helpers/imports to debug + test-support capabilities;
  production methods and their denial guards are unchanged. Add an explicit
  release-only test asserting `Client` denial for the synthetic endpoint API.
- Green: `cargo fmt --all --check`; full ordinary `cargo test --locked --all-targets
  --no-default-features`, `--all-features`, and `--release --all-targets --all-features`;
  `cargo clippy --locked --all-targets --all-features -- -D warnings`.
  Historical external run artifacts are not in the repository. Ignored integration
  tests remain fixture-gated and are not claimed by these runs.
- Independent source review found no blockers;
  no production bypass, broad target gate or unrelated code changes. Acceptance
  satisfied; archived. HTTP selectors/assets/CI remain separately scoped issues.
