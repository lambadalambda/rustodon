# Fence browser reauthentication and recovery

## Summary

Prevent stale credential checks from authorizing writes after recovery, and rate-limit sensitive password challenges.

## Requirements

- Couple credential verification to session issuance and password changes using atomic checks or a validated credential generation.
- Use shared account/IP reauthentication limits before bcrypt across sensitive settings routes.
- Keep transaction fencing and abuse limits in separate topical commits.

## Acceptance Criteria

- Barrier-controlled login/reset and password-change/reset races cannot create authority based on the recovered password.
- Incorrect-password attempts across password and 2FA settings share a bounded budget; legitimate reauthentication works.

## Notes

- Findings R03, R13 in [the essential feature parity review](../essential-feature-parity-review.md), baseline `1861d33`.
- Review-only discovery: no implementation fix is included in the review commit. End-to-end scenarios remain to be executed unless the report explicitly records a parser reproduction.

## R03 implementation and evidence

- Replaced user-ID-only session issuance with a typed browser-authentication proof. The private proof retains the checked bcrypt hash as a credential version (fresh salt on replacement, including same-plaintext resets). Session issuance and security password changes compare that version under the same user-row lock used by recovery, holding it until commit.
- Security password changes no longer call administrative reset by email. Password change, administrative reset, and token recovery share one transactional password-replacement/revocation helper. No public Mastodon schema changes.
- `browser_recovery_fences` uses two barriers and separate database connections: check credentials and create a pre-reset session, commit token recovery, then resume stale session issuance/password change. Both must return `Unauthorized`; recovered password remains intact and session/live-token counts are zero. Fresh login and legitimate password change succeed afterward. The test has a two-minute deadlock guard, not timing-based scheduling.
- RED against unfenced code: `fencelogin: rejected=false, recovered_password=true, sessions=1, live_tokens=1`; `fencechange: rejected=false, recovered_password=false, sessions=0, live_tokens=0`. Assertion `[false, false] != [true, true]`; 0 passed, 1 failed. Initial test compilation exposed private `SecretText::as_str`; the test was corrected to inspect fixture SQL before obtaining the behavioral RED.
- GREEN: both races `rejected=true, recovered_password=true, sessions=0, live_tokens=0`; 1 passed. Existing `browser_authentication`, `password_recovery`, and `account_settings` cases each passed (1 test per case).
- Independent read-only review: no correctness/architecture blockers; typed authority, row locking, shared replacement helper, and deterministic schedules verified. Optional follow-ups: stale login currently maps safe fence rejection to HTTP 500, and public authentication metadata could eventually become accessors (session authority already uses only the private proof).

All Rust execution used an isolated worker and task-owned workspace, Rust 1.97.1. Exact RED/GREEN race command (executed before/after the fix):

```sh
CARGO_BUILD_JOBS=4 tools/mastodon-fixture differential-test browser_recovery_fences
```

Validation commands, in that same task-owned workspace:

```sh
cargo fmt
CARGO_BUILD_JOBS=4 tools/mastodon-fixture differential-test browser_authentication
CARGO_BUILD_JOBS=4 tools/mastodon-fixture differential-test password_recovery
CARGO_BUILD_JOBS=4 tools/mastodon-fixture differential-test account_settings
CARGO_BUILD_JOBS=4 cargo clippy --locked --all-targets --all-features -- -D warnings
```

Clippy initially flagged the newly added timeout's `from_secs(120)`; changed to `from_mins(2)` and rerun. The fixture harness uses disposable PID-named containers/clones, not live data. The existing read-only pinned Mastodon source checkout was verified at `1440d55b139e39ec722c2a3db7f60b66cd889048` and linked read-only into the task workspace; the prescribed pinned Mastodon 4.6.5 checkout was absent locally and was not fetched. New race assertions are Rust regression proofs, not a new Rails differential claim.

Source transfer included tracked source only and excluded Git metadata, build targets, instance state, backups, environment files, and unrelated user configuration; it did not delete destination files. Only changed Rust source files formatted externally were retrieved. No local builds/tests/format/lint, live instance credentials, OAuth client fixes, or upstream edits occurred. Issue indexes were intentionally untouched.

## R13 implementation and evidence

- Added one shared reauthentication policy: **10 attempts/user/hour** and **25 attempts/IP bucket/5 minutes**, independent keys (not a user/IP pair). Existing IP normalization, PostgreSQL rate-limit transactions, deterministic lock ordering, fail-closed errors, expiry, and response headers are reused. Successful challenges also consume budget; switching route/session/worker does not clear it. This is a separate settings budget, not a change to login policy.
- After session/CSRF validation and before password verification, all six sensitive-settings handlers reserve that budget: security password change, 2FA disable, recovery-code regeneration, OTP setup, OTP confirmation, and account deletion. Aliases share the guarded handlers. Denials render HTTP 429 with rate-limit and Retry-After headers. No public Mastodon schema change.
- `browser_reauthentication_limits` starts **two independent WebStates/database pools/server tasks in one process**, with one fixture CSRF signing secret. Ten incorrect guesses rotate across six routes, both servers, and IP addresses; all six subsequently reject both correct and incorrect passwords. A test-support-only atomic counter observes entry to the actual `verify_password`/bcrypt boundary: limited requests do not increment it. Three more fixture users exhaust one IP's 25-attempt budget across both servers. A fresh user/IP can legitimately reauthenticate; SQL expiry of fixture windows restores legitimate access without sleeping.
- RED: after ten incorrect challenges, `/settings/security` still returned **422 instead of 429**; 0 passed, 1 failed. Initial test compilation required converting a harness error to the test's boxed error; corrected before the behavioral RED. GREEN: 1 passed; output confirms all six routes blocked before bcrypt, shared user/IP budgets, expiry, and legitimate reauthentication.
- Independent read-only review found no R13 correctness/architecture blockers. The routine no-case fixture invocation now enables `test-support`, as the named invocation does, so this regression is not silently omitted. Remaining nonblocking test constraint: exhaustion assertions use the existing real fixed-window clock and can cross a five-minute/hour boundary; explicit expiry is deterministic. Bare nonproduction `WebState::new` still has the pre-existing local-limiter fallback; production queue/writer wiring installs shared enforcement and shared errors never fall back locally.

Exact RED/GREEN command, both before and after the limiting fix:

```sh
CARGO_BUILD_JOBS=4 tools/mastodon-fixture differential-test browser_reauthentication_limits
```

The named harness runs `cargo test --locked --features test-support --test differential browser_reauthentication_limits -- --ignored --exact --nocapture --test-threads=1` with its disposable fixture URLs. GREEN output:

```text
user budget: 10 challenges shared across six routes/two instances/IP changes; all six blocked before bcrypt
IP budget: 25 challenges shared across three users/two instances; blocked before bcrypt; expiry and legitimate reauthentication work
test result: ok. 1 passed; 0 failed
```

Final external checks in the same task-owned workspace:

```sh
cargo fmt --check
CARGO_BUILD_JOBS=4 tools/mastodon-fixture differential-test browser_recovery_fences
CARGO_BUILD_JOBS=4 tools/mastodon-fixture differential-test browser_two_factor_management
CARGO_BUILD_JOBS=4 tools/mastodon-fixture differential-test account_settings
CARGO_BUILD_JOBS=4 cargo test --locked --all-targets --all-features
CARGO_BUILD_JOBS=4 cargo clippy --locked --all-targets --all-features -- -D warnings
```

Results: all three restored-fixture cases passed (1 each); full ordinary Cargo suite **406 passed, 0 failed, 127 ignored**; rustfmt and warnings-denied Clippy passed. Ignored integration cases are not claimed executed except the explicit fixture cases recorded here and under R03. `cargo fmt` ran remotely before verification; only changed formatted Rust files were retrieved. R03/R13 acceptance is implemented and verified, but this detail remains linked from the open index because the requested scope excludes index/archive edits. No new Rails differential behavior is claimed for the Rust-only security regressions.
