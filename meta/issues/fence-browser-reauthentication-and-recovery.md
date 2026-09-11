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

All Rust execution is on `lain@secunda.local`, isolated mirror `/home/lain/rustodon-parity/reauth`, Rust 1.97.1. Exact RED/GREEN race command (executed before/after the fix):

```sh
ssh lain@secunda.local 'bash -lc "cd /home/lain/rustodon-parity/reauth && CARGO_BUILD_JOBS=4 tools/mastodon-fixture differential-test browser_recovery_fences"'
```

Remote validation commands, in that same directory:

```sh
cargo fmt
CARGO_BUILD_JOBS=4 tools/mastodon-fixture differential-test browser_authentication
CARGO_BUILD_JOBS=4 tools/mastodon-fixture differential-test password_recovery
CARGO_BUILD_JOBS=4 tools/mastodon-fixture differential-test account_settings
CARGO_BUILD_JOBS=4 cargo clippy --locked --all-targets --all-features -- -D warnings
```

Clippy initially flagged the newly added timeout's `from_secs(120)`; changed to `from_mins(2)` and rerun. The fixture harness uses disposable PID-named containers/clones, not live data. The pinned upstream source is the existing read-only remote checkout `/home/lain/repos/rustodon/target/mastodon-v4.6.5` at `1440d55b139e39ec722c2a3db7f60b66cd889048`, symlinked in the mirror's target; absent local `/workspace/rustodon/target/mastodon-v4.6.5` was not fetched. New race assertions are Rust regression proofs, not a new Rails differential claim.

Sync uses `rsync -az --exclude='/.git' --exclude='/target/' --exclude='/.local-instance/' --exclude='/.local-instance-backups/' --exclude='/.env*' ./ lain@secunda.local:/home/lain/rustodon-parity/reauth/`, without `--delete`. Only changed Rust source files formatted remotely are retrieved. No local builds/tests/format/lint, live instance/env secrets, OAuth client fixes, upstream edits, or remote user tracked-config changes. Issue indexes are intentionally untouched.
