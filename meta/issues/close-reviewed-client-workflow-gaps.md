# Close reviewed client workflow gaps

## Summary

Repair qualified local mentions, hashtag stream fan-out, and public OAuth/discovery contract inconsistencies.

## Requirements

- Normalize fully qualified local mention handles to local account identity.
- Align followed-hashtag user-stream lifecycle events with supported home timeline eligibility.
- Allow public OAuth clients to revoke their own tokens while retaining application ownership checks.
- Only advertise implemented OAuth response modes and reject unsupported requests explicitly.
- Implement each client workflow in a separate topical commit, not one broad rewrite.

## Acceptance Criteria

- Qualified and short local mentions grant identical direct-post access and notifications on create/edit.
- A hashtag-only follower receives eligible create/edit/delete events corresponding to REST home membership.
- Public PKCE authorize/exchange/revoke rejects the revoked bearer and cannot revoke another application token.
- Advertised response modes work on approval and denial; unsupported modes have a tested rejection contract.

## Notes

- Findings R11, R12, R14, R15 in [the essential feature parity review](../essential-feature-parity-review.md), baseline `1861d33`.
- Review-only discovery: no implementation fix is included in the review commit. End-to-end scenarios remain to be executed unless the report explicitly records a parser reproduction.

## R11 — qualified local mentions: implemented and verified

- The shared create/edit mention resolver maps the configured local account domain
  (case-insensitively) to the NULL domain used by local account rows. `WebState`
  supplies that domain to its writer; the web origin is deliberately not used.
  Remote-domain lookup, eligibility checks, and account-ID deduplication are unchanged.
- `tests/qualified_local_mentions.rs` exercises HTTP create and edit, durable outbox
  dispatch, the real core notification worker, recipient status-show and notification
  REST reads, and persisted mention/notification rows. Its 12 cases cover short,
  qualified, uppercase qualified, and combined short/qualified spellings on both
  create and adding a recipient through edit. Nonlocal and web-host-only handles
  do not grant local access; anonymous access and pre-edit recipient access are denied.
- The fixture command retains its existing schema/saved-status tests and includes
  R11 in its default run; `schema-read-test qualified_local_mentions` selects only R11.
- Read-only upstream inspection: canonical `/workspace/rustodon/target/mastodon-v4.6.5`,
  actual `/home/lain/repos/rustodon/target/mastodon-v4.6.5`, verified revision
  `1440d55b139e39ec722c2a3db7f60b66cd889048`. `ProcessMentionsService#scan_text!`
  normalizes local domains to nil before account lookup. No source fetched or changed.

### Exact remote TDD evidence

All commands below ran through `ssh lain@secunda.local bash -s`, from
`/home/lain/rustodon-parity/clients`, with `export CARGO_BUILD_JOBS=2`.
New tests were staged before synchronization. Only tracked changed files were
synced (excluding `.git`, `target`, `.local-instance*`, `.env*`, no `--delete`);
the workspace already contained the tracked source. An initial full tracked sync
stopped at the existing dangling `public/500.html` symlink, so changed-file sync
was used. Only intentionally formatted source was retrieved.

**RED, before production changes:**

```sh
tools/mastodon-fixture schema-read-test qualified_local_mentions > /tmp/r11-red.log 2>&1
```

Exit 101, one failing integration test (10.89s). Both create and edit produced
`(mentions, notifications, recipient HTTP, visible notifications) = (0, 0, 404, 0)`
for qualified and uppercase qualified local handles instead of `(1, 1, 200, 1)`.
Short and mixed spellings passed; negative cases remained denied. An earlier test
compilation attempt lacked reqwest's optional JSON methods; the test was corrected
to use the project's existing string-body JSON approach before this behavioral RED.

**GREEN:** the normal command hit Docker Hub's anonymous pull rate limit (exit 125)
while inspecting the PostgreSQL index. The pinned child images were already cached.
A temporary, untracked harness copy skipped only registry manifest inspection/pull,
retaining local pinned image digest and linux/amd64 verification, static fixture
checksums, and the original PID-isolated rootless database lifecycle:

```sh
sed '/^  manifest=$(run_podman manifest inspect /,/^  run_podman pull --quiet /d' \
  tools/mastodon-fixture > tools/.r11-cached-fixture
sh tools/.r11-cached-fixture schema-read-test qualified_local_mentions > /tmp/r11-green-final.log 2>&1
cargo test --locked --lib > /tmp/r11-unit.log 2>&1
rustfmt --edition 2024 --check src/mastodon/write_repository.rs src/web.rs tests/qualified_local_mentions.rs
sh -n tools/mastodon-fixture
cargo clippy --locked --all-features --lib --test qualified_local_mentions -- -D warnings
```

- Final integration GREEN: **1 passed**, all 12 matrix cases, 10.90s; an earlier
  post-fix run also passed. Fixture resources cleaned up by the harness.
- Library tests: **236 passed, 2 ignored**, 2.75s. The existing mention-parser unit
  also passed separately via `cargo test --locked --lib status_mentions_preserve_local_and_remote_account_shapes`.
- Changed-source formatting, shell syntax, and targeted all-features Clippy passed.
  Default-feature targeted Clippy was blocked by pre-existing `unused_self` in
  `src/paperclip.rs:1129`; no unrelated fix or lint suppression was added.
- Independent read-only `explore` correctness and architecture/DRY review approved
  the supplied exact implementation diff and inspected test, with no substantive
  blockers. Reviewer could not independently run Git/SSH; execution evidence was
  supplied by the implementing agent.

### Limits and remaining scope

This is isolated fixture client/worker evidence, not a fresh Rails differential,
production-role permission test (fixture owner performs writes/worker processing),
live browser/mobile or federation peer test, full all-target release gate, or a
fresh registry index verification on GREEN. Switching an already-mentioned recipient
short → qualified → short with notification-count stability is optional follow-up
coverage; this matrix tests new mentions on create and newly added mentions on edit.
R12/R14/R15 remain untouched and open; no browser credential functions or issue
indexes changed.
