# Bootstrap the Rust workspace and quality gates

## Summary

Create the smallest maintainable Rust workspace on which the compatibility
work can proceed. This issue establishes tooling and project boundaries but
does not implement Mastodon behavior.

## Requirements

- Create a Cargo workspace using the current stable Rust edition.
- Start with one application crate unless a second crate has an immediate,
  tested reuse boundary.
- Provide binary entry points or subcommands for `web`, `worker`, and
  administrative commands without implementing their full behavior.
- Add formatting, linting, unit-test, and dependency-policy commands suitable
  for local development and CI.
- Deny unsafe Rust unless a documented compatibility need is discovered.
- Add a minimal CI workflow that runs formatting, Clippy, and tests.
- Document the supported Rust toolchain and local commands in the README.

## Acceptance Criteria

- `cargo fmt --check` succeeds.
- `cargo clippy --all-targets --all-features -- -D warnings` succeeds.
- `cargo test --all-targets --all-features` succeeds.
- The application binary prints useful help for the planned process modes.
- CI runs the same checks from a clean checkout.
- No database schema or production feature is introduced by this issue.

## Notes

- This is the first implementation issue and blocks all other Rust code.
- Prefer a compact workspace over speculative crates and abstraction layers.
- Use red-green-refactor for executable behavior introduced by the bootstrap.
