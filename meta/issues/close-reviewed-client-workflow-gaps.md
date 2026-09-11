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
R12/R14/R15 were not included in R11; no browser credential functions or issue
indexes changed.

## R12 — followed-hashtag stream eligibility: implemented and verified

- `record_status_stream_events` now includes a deduplicated hashtag-follow recipient
  branch shared by local/remote create, edit, and delete. It mirrors REST home's
  hashtag eligibility: public originals only, active local recipients, unsuspended
  and unsilenced authors, bilateral author/viewer blocks, viewer mutes, blocked/muted
  active mentions, and viewer domain blocks. Account-follow-specific language, reply,
  and exclusive-list restrictions are not incorrectly imposed on hashtag membership.
- Ordinary local/remote status-delete callers already tombstone statuses but retain
  tags. Only the tombstone
  check is relaxed for hashtag delete recipients; visibility and policy exclusions
  remain enforced. Existing follower/author branches and payload authorization stay
  unchanged. No R17/R18 queue changes.
- `tests/followed_hashtag_streams.rs` removes the viewer's account follow, follows
  two tags, and compares REST home membership with exact cumulative stream events
  for create/edit/delete. Eleven scenarios cover public eligibility plus unlisted,
  private, direct, viewer/author blocks, author mute, blocked/muted mentioned account,
  silenced author, and an unfollowed tag. Two matching tags yield only one event.
  A real WebSocket additionally receives `user` envelopes for `update`,
  `status.update`, and ID-only `delete`, including the edited content.
- Read-only upstream source: canonical `/workspace/rustodon/target/mastodon-v4.6.5`,
  actual `/home/lain/repos/rustodon/target/mastodon-v4.6.5`, revision rechecked as
  `1440d55b139e39ec722c2a3db7f60b66cd889048`. Inspected
  `FanOutOnWriteService#deliver_to_hashtag_followers!` and its public-recipient
  dispatch. No source fetched/changed; no fresh Rails differential claim.

### Exact remote RED/GREEN evidence

All execution ran through `ssh lain@secunda.local bash -s`, in
`/home/lain/rustodon-parity/clients`, with `export CARGO_BUILD_JOBS=2`.
The new test was staged before syncing tracked changed files only, excluding
`.git`, `target`, `.local-instance*`, `.env*`, without `--delete`.

Docker Hub's previously confirmed anonymous limit was not retried. The same
transparent temporary cached-child workaround as R11 retained pinned local
linux/amd64 image/digest verification, fixture checksums, and PID-isolated rootless
fixture setup/cleanup, skipping only registry manifest inspection and pull:

```sh
sed '/^  manifest=$(run_podman manifest inspect /,/^  run_podman pull --quiet /d' \
  tools/mastodon-fixture > tools/.r12-cached-fixture
# RED before the production SQL change:
sh tools/.r12-cached-fixture schema-read-test followed_hashtag_streams > /tmp/r12-red.log 2>&1
# GREEN after the production SQL change:
sh tools/.r12-cached-fixture schema-read-test followed_hashtag_streams > /tmp/r12-green.log 2>&1
rustfmt --edition 2024 --check src/mastodon/write_repository.rs tests/followed_hashtag_streams.rs
sh -n tools/mastodon-fixture
cargo clippy --locked --all-features --lib --test followed_hashtag_streams -- -D warnings
cargo test --locked --lib > /tmp/r12-unit.log 2>&1
sh tools/.r12-cached-fixture schema-read-test > /tmp/r12-schema-all.log 2>&1
```

- **RED:** exit 101, one failed test (9.97s). Public create/edit were present in REST
  home but emitted no events; delete also emitted none. All ten negative scenarios
  remained absent from home and emitted no events throughout the lifecycle.
- **GREEN:** one passed test (11.10s), all 33 matrix checkpoints plus the three real
  WebSocket lifecycle envelopes.
- Changed-source formatting and shell syntax passed. Targeted all-features Clippy
  passed after adding the standard `too_many_lines` allowance to the enlarged SQL
  function; no query-only extraction or unrelated lint fixes.
- Library tests: **236 passed, 2 ignored** (2.58s).
- Full default restored-schema command passed all **40 tests** across four fresh
  fixture databases: schema **37** (19.47s), saved-status authorization **1** (9.08s),
  R11 qualified mentions **1** (11.32s), R12 hashtag streams **1** (11.80s).
  The SSH call exceeded its 120-second foreground wait; the remote run continued
  and its completed log and harness cleanup were checked before returning.
- Independent read-only `explore` correctness and architecture/DRY review found no
  substantive blockers. It checked the REST exclusions, shared callers, retained
  delete tags, recipient deduplication, and payload authorization against source;
  execution evidence was supplied by the implementing agent.

### Limits / optional follow-up

This proves current eligible recipients, not remembered historical audiences.
Removing the last followed tag, unfollowing a tag, or changing policy before deletion
can prevent later events reaching a former recipient; historical client-cache cleanup
needs separate requirements/coverage. Remote-author domain blocks, suspended authors,
disabled/suspended/nonlocal recipients, and account-follow-plus-tag overlap were source-reviewed
but not individually exercised by this matrix. Fixture owner writes do not prove
production-role permissions. No live peer/browser/mobile, fresh registry verification,
or complete all-target release gate is claimed. The registry-cache harness repair
belongs to the parent task and is not included here. R14/R15 were not included in R12.

## R14 — public PKCE own-token revocation: implemented and verified

- OAuth credential parsing now accepts an omitted form secret or empty Basic
  password. Authentication remains in the repository: public clients identify
  themselves; confidential clients must supply a nonempty matching secret for
  revocation. The other OAuth grant handlers retain their existing authentication.
- Revocation requires the token's application ID to equal the identified application's
  ID exactly, rejecting cross-application and applicationless tokens before writes.
  Existing token-row locking, transactionality, push-subscription cleanup and stream
  token-kill recording are preserved. No browser credential functions changed.
- `tests/public_oauth_revocation.rs` runs real HTTP authorization GET, CSRF-protected
  consent POST, S256 code exchange, successful bearer use, revocation and rejected
  bearer use. Two public clients revoke without a secret (form identity and Basic
  empty password). Public-to-public and public-to-confidential revocation return 403
  while victim bearers remain usable. Confidential missing/empty/wrong form secrets
  fail; correct confidential Basic succeeds. Applicationless-token revocation fails
  and its bearer remains usable.
- Fixture setup seeds applications and an authenticated consent session; it does not
  exercise or modify password authentication. Redirect following is disabled, so no
  callback origin, live credentials, tunnel, or remote peer is contacted.

### Exact remote RED/GREEN evidence

Parent cache-safe fixture commit `52fa99a` was first cherry-picked as `183a8e0`.
The real harness was used throughout: **no temporary bypass, registry retry,
credentials, unpinning, or harness verification repair in the R14 commit**.
All execution ran through `ssh lain@secunda.local bash -s`, from
`/home/lain/rustodon-parity/clients`, with `export CARGO_BUILD_JOBS=2`.
The new test was staged before syncing tracked changed files only, excluding `.git`,
`target`, `.local-instance*`, `.env*`, without `--delete`. The parent fixture fix's
tracked files were also synchronized before RED.

```sh
# RED before the production OAuth changes:
tools/mastodon-fixture schema-read-test public_oauth_revocation > /tmp/r14-red.log 2>&1
# GREEN after the production OAuth changes:
tools/mastodon-fixture schema-read-test public_oauth_revocation > /tmp/r14-green.log 2>&1
cargo test --locked --lib > /tmp/r14-unit.log 2>&1
cargo clippy --locked --all-features --lib --test public_oauth_revocation -- -D warnings
rustfmt --edition 2024 --check src/mastodon/write_repository.rs src/web.rs tests/public_oauth_revocation.rs
sh -n tools/mastodon-fixture
tools/mastodon-fixture schema-read-test > /tmp/r14-schema-all.log 2>&1
```

- **RED:** exit 101, one failing test (9.73s). Both public clients successfully
  authorized/exchanged but revocation returned 403 `unauthorized_client`, leaving
  bearers accepted (200). Confidential revocation returned 200 and bearer use 401.
- **GREEN:** one passed test (8.95s), including both public-client revocation forms,
  complete HTTP consent/S256 lifecycle, cross-app denial and surviving victim bearers,
  confidential authentication controls, and applicationless-token protection.
- Library tests: **236 passed, 2 ignored** (2.62s). Targeted all-features Clippy,
  changed-source formatting and shell syntax passed.
- Full real restored-schema command: **41 passed** across five fresh fixture
  databases: schema **37** (20.87s), saved-status authorization **1** (9.47s),
  R11 **1** (9.85s), R12 **1** (13.05s), R14 **1** (10.68s). The SSH call exceeded
  its 120-second wait; the remote run continued, and its completed log and fixture
  cleanup were checked before returning.
- Independent read-only `explore` correctness and architecture/DRY review approved
  the supplied change scope and inspected source/test, including all shared OAuth
  parser callers. It found no substantive blockers; execution evidence was supplied
  by the implementing agent because the reviewer lacked Git/SSH tools.

### Limits / optional follow-up

Fixture owner writes do not prove production-role permissions; the consent session
was seeded, not obtained by password login. No fresh Rails differential, live
browser/mobile/peer, or full all-target release gate is claimed. The pinned source
revision was rechecked remotely at `/home/lain/repos/rustodon/target/mastodon-v4.6.5`
(canonical `/workspace/rustodon/target/mastodon-v4.6.5`),
`1440d55b139e39ec722c2a3db7f60b66cd889048`; no source fetched or changed.

Optional follow-up: explicit HTTP negatives for confidential empty-password Basic
and public `client_credentials` requests. Source review also noted pre-existing
handling of an anomalous confidential application with an empty stored secret in
code exchange; that separate hardening was not included. R15 remains untouched/open,
and no browser credential functions or issue indexes were changed.
