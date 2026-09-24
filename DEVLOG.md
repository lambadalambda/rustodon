## 2026-09-24 — Worker lane green

- Full worker lane 122/0, and the CLI readiness step after it now passes too.
  Clippy clean.
- Product fixes: a poll startup that finds no writer connection keeps its fixed
  lease (the retry path used to clear it); quote Accept/Reject delivery compared
  the remote requester with the local sender, so it never sent.
- Test isolation was the rest: the account purge test rewrote every remote inbox
  (forwarding then skipped the "source" inbox), an outbox row without
  max_attempts, activity test users without keys (preflight refused the worker),
  and the fixture server read only `Content-Length` (hyper sends lowercase).

## 2026-09-24 — Worker lane second pass

- Full worker lane 118/4, then focused fixes; only two tests remain, both needing a
  behavior decision (poll startup claim order; legacy quote counting vs Mastodon).
- Product fixes: replay forwarding before refetch (after policy), quoted_update
  cleanup order and late-job guard. Test fixes: protocol-0 fixture account, lazy
  status_stats, activation marker, poll expiry assertion, racy outbox inserts.
- Deployed c2ae939 (outbox timestamp fix).

## 2026-09-24 — Worker lane repair, first pass

- Full worker lane from 93/29 to 105/17, then focused fixes 7/7. One product bug
  (outbox dispatched_at before created_at) and several outdated tests; a filtered
  `worker-test filter` case with several libtest filters makes reruns cheap.
- Remaining failures are listed in the worker-lane issue; they need real analysis
  (late notification job, forwarding replay, quote lifecycle, poll startup, test
  isolation).

## 2026-09-24 — Misskey null content, split-domain actors, worker lane state

- Remote Notes with `"content": null` (Misskey media-only) are accepted as empty
  text; one shared helper serves inbox validation and the writer.
- Split-domain actors (actor on mastodon.bsd.cafe, handle acct:jae@bsd.cafe) now
  resolve with Mastodon's two-step WebFinger check and are stored under the
  confirmed domain. An independent security review caught three blockers before
  commit: host-level domain policy, host-based key-resolution domain, and the
  known-key ingress comparison. Red/green `worker-test split_domain` (the first
  red was invalid: severity 2 is `noop`, not `suspend`).
- The full worker lane fails on main: 93 passed / 29 failed on both baseline
  31244a8 and the change; filed as its own issue. Several failures only appear
  in the full run, so test isolation is part of the problem.
- Deployed 341e784 before these fixes.

## 2026-09-23 — Split the three largest files

- Pure moves, one area per commit, each gated by check, strict Clippy and lib
  tests on the NAS; an independent review diffed every commit (only mechanical
  edits). web.rs 23.6k -> 12.9k, worker.rs 10.9k -> 4.1k, write_repository.rs
  24.8k -> 13.4k lines (all including tests).
- New modules: web/{federation,oauth,browser,rate_limit,routes,frontend,
  request_params,stream_socket,media}, worker/{push,delivery,pull,ingress,
  poll_expiration}, write_repository/{auth,moderation,remote_ingest,remote_note,
  purge}. Interim pattern: pub(super) items and glob imports marked with allows;
  change a child to explicit `use super::{...}` when it stabilizes.
- One shared `lock_account_advisory` replaces ten copies of the raw account-id
  lock (same SQL and keys). Hashed and namespaced locks stay per call site.

## 2026-09-23 — Dead letters, small fixes, AGPL

- Production dead letters (174) are mostly remote 404/5xx. Two real bugs filed:
  split-domain actors fail WebFinger verification, and Misskey notes with null
  content are rejected.
- Fixed stale-login session denial returning 500. Tag atom identifiers pass
  their first run (3 unit, `worker-test update_versions` 2/2).
- The worker lane's CLI readiness step still fails on the loaded NAS: startup's
  media capability check (15 conversions, one 60 s deadline) times out under
  ~65% I/O pressure. The same check passes in an idle container.
- Licensed under AGPL-3.0-or-later, like Mastodon.

## 2026-09-23 — rustls advisory, reply peer scenario, partial sweep

- cargo-deny flagged RUSTSEC-2026-0285 (rustls 0.23.43); updated to 0.23.45.
- Added `peer-replies` (two-way reply threading and `/context`). Not passed yet:
  two NAS attempts hit the 600 s runner deadline during fixture restore.
- Static and ordinary gates pass on ff825be; heavy fixture lanes remain for a
  quiet NAS (see the v1 external acceptance issue for the exact list).

## 2026-09-23 — Review fixes and a NAS runner for fixture lanes

- Review of the whole tree found the web UI, instance and NodeInfo routes
  returned 503 whenever the activity count query failed. The cache now serves
  the last good counts for up to 24 h, else zeroes (reverses the fail-closed
  choice). Also: activity prune failure no longer skips operational cleanup,
  tracked requests skip the user-row lock when not due, and an unsigned
  signature `expires` can only shorten the Date window.
- Fixture lanes run on the NAS from a derived tools image with a podman client
  wrapper for the host socket, host networking and identical workspace paths.
  Sync must not preserve Mac mtimes (tar did, so cargo reused stale builds);
  a checksum rsync without `--times` works.
- New `worker-test instance_activity` case. Red/green on NAS PG14: activity
  storage 8/8, maintenance 2/2, signatures 8/8, startup activity test passes.
  Strict Clippy (all targets, after test-only lint fixes), fmt, ordinary
  all-feature and default suites pass; preflight 12/12 needs `--cap-drop=all`
  (root bypasses the 0600 check).
- Not proven: `web_probes_and_trusted_forwarding_are_operational` (5 s health
  wait) and the worker lane's CLI readiness step (6 s) timed out while another
  job saturated NAS disk I/O. Rerun before claiming the startup/worker lanes.

## 2026-09-19 — Combined b64e94d verification, partial / paused

- Exact tracked b64e94d archive on native NAS Linux x86-64; all 6485 source
  hashes matched after execution. Sequential bounded tools runs, disposable PG14,
  no production/deployment/push or application/harness changes.
- Passed default-feature all-target ordinary suite, fmt, Linux harnesses, static
  fixture/media checks, real codec 3/3, full pinned source 12/12. Strict Clippy
  failed test-helper lint; all-feature debug/release hit missing external `kill`;
  cargo-deny absent. Initial read-only scratch failure retained, scratch corrected.
- Focused activity storage 6/7 (one lock-wait timeout), maintenance worker 1/1.
  Upgrade/lifecycle/upload HTTP/bootstrap blocked by task role setup: initial
  privileged owner, then missing NOINHERIT. Paused after two setup variants;
  these are not application-regression proof or full named DB gate results.
- Named peer-public verified cached prerequisites then blocked on host cargo
  missing, before peer resource creation. No peer/browser/differential acceptance
  claim; broader matrix not run. Task PG/container/anonymous volume/network cleanup
  independently confirmed; source/logs/tools image/caches intentionally retained.
- Detailed commands, every exit, limitations and cleanup in
  `target/combined-b64e94d-evidence/evidence/` and the existing open isolated-worker
  issue. No TDD cycle for verification-only work; nested review unavailable.

## 2026-09-19 — Instance activity browser acceptance and focused closure

- Parent review `980993` found no high-severity blocker; archived activity parent,
  recording, cache and browser issues using combined reviewed source/restricted
  DB/worker/main-HTTP evidence plus real pinned browser on exact `10ddf66`.
- Migration 6 empty history and confirmed eligible TODAY login publish 0. Setup
  moved that real helper-recorded bucket/member to yesterday: sidebar/v2/initial
  1 and NodeInfo 1/1; limited sidebar/v2/initial 0, NodeInfo 1/1. Reloads passed.
  This is simulated history, not observed midnight, real backfill or deployment.
- Exact DISTINCT instead of approximate HLL is intentional; returning tracking
  covers interactive HTML/credentials/settings/session hooks, not full lifecycle
  parity. Browser library 3 passed / 7 ignored, focused source 1 passed; closure
  harness 2 + shared 8 passed. No ignored-test or full-matrix pass claim.
- Sanitized exact-source/hashes/screenshots/grants and separate final cleanup proof
  remain under `target/activity-browser-evidence/evidence/`. Deferred medium
  cleanup-status masking and missing negative observer unit tests are recorded in
  the browser issue. Wider combined sweep remains pending; no production push.

## 2026-09-19 — Activity review-2 fixture reliability follow-up (tests only)

- Parent accepted production source with no high-severity findings. Addressed
  the medium fixed-date reliability finding without production edits: exact
  midnight is now 23:59:59 UTC two database-clock days before the test, safely
  separate from today's activations. Expired renewal uses an expiry explicitly
  before PostgreSQL's clock; strict-24h and union-window simulations derive their
  reference time from that same database, not January/September 2026 constants.
  Exact midnight, strict 24h, TTL-duration and 28/168-day boundary checks remain.
- Reused the existing HTTP fixture for due `/settings/profile` and `/auth/session`
  requests alongside `/home`. Each must record membership and previous/current
  sign-in timestamps without incrementing sign-in count. Existing media GET/HEAD
  immutability and ordinary-bearer nontracking assertions remain. No browser
  infrastructure, aggregate, prune, auth-policy or other production change.
- Final serial restricted-role PG14.23 / NAS7203 reruns: activity **4 passed**
  (`r2-final-activity.log`, 10.15s); media plus interactive HTTP **1 passed**
  (`r2-final-http.log`, 27.93s); focused strict library/media-test Clippy passed
  (`r2-final-clippy.log`); local formatting/diff checks passed. These are the
  focused reruns, not a new full-matrix or bootstrap claim.
- Same task workspace and bounded tool runner as the prior slice: 4 CPU/6 GiB/
  512 PIDs/870s, 900s outer bound; newly created internal network and PG14
  container with 1 CPU/512 MiB/128 PIDs/3600s lifetime, no published ports.
  Initial provisioning caught the PostgreSQL temporary startup socket; roles
  and a fresh restore were retried after TCP readiness. That setup failure and
  initial Clippy function-length rejection are retained; a test-only length
  allowance keeps the single HTTP scenario together. No production access.
- Only `src/activity/tests.rs` and `tests/support/local_upload_http.rs` changed
  relative to the prior tested source manifest; all other source/schema/grant
  hashes remain identical. Final worker/local **22/22 hashes matched**.
  Prior evidence was not overwritten: new logs, tests-only patch, manifests and
  cleanup evidence are in ignored `target/daily-activity-evidence/review2/` and
  remote `evidence/r2-*`. Task container/anonymous volume/network removed;
  shared caches and workspace retained. Left uncommitted for parent review 2.

## 2026-09-19 — Transactional daily activity storage/auth implemented, review pending

- Continued the approved `16c751c` slice with migration **6**, two Rust-owned
  tables (`activity_buckets(day, expires_at)` and exact
  `activity_members(day, user_id)`), and no FKs/public historical-state joins.
  Bucket renewal and expired-member removal serialize under a bucket lock.
  Every eligible duplicate resets the shared expiry from the actual PostgreSQL
  clock **after acquiring that lock**, not the earlier event/TOTP timestamp.
  Later maintenance pruning must lock buckets and remove both tables' rows in
  one transaction; no new job or pruning implementation in this slice.
- Confirmed+approved activation is transactional for confirmed creation,
  approved confirmation and fresh admin bootstrap; future approval reuse is
  tested by simulation. Bootstrap retains the admin ID across migration and
  records once afterward in the install transaction. Its exact baseline now
  validates that one activation; reruns do not refresh it. Complete
  password/TOTP/backup session creation records after the final credential fence
  and rereads confirmed locality. No count at the earlier login audit, no auth
  transaction merge, no new global disabled/suspended/approval policy.
- Strict nil or **older than 24h** retained claims hold the user lock and commit
  previous/current sign-in timestamps with membership, without incrementing
  `sign_in_count`. Explicit hooks cover frontend HTML, `verify_credentials`,
  required browser settings and session requests. This is a bounded subset of
  pinned controller tracking, not broad API/bearer tracking. Media GET/HEAD
  cookie auth and repository session reads remain mutation-free.
- Reader gains **SELECT only** on the two tables; known writer gains operational
  CRUD. Updated bootstrap, migration grant discovery, exact operational/writer
  validation and fixture grant/downgrade profiles; public grants unchanged.
  Derived PG14.23 catalogs directly from migration SQL: v5 reproduced
  `99b8375f5c773fc1200acc782a25ec9d92e237699b165c2f961c04bdea521f99`, v6 is
  `60f2d809c3d122ff8d908f315005db00cf7179dfdc336388bd05beaa5bf652a6`.
- Exact clean pinned Mastodon source verified with
  `tools/mastodon-fixture verify-source target/mastodon-v4.6.5`. Only explicit
  tracked files needed by the focused source test were archived read-only onto
  the worker, not `.git` or build output. ActiveSupport **8.1.3** matches its
  Gemfile.lock. The 7203 tools image has no Ruby gem source; read the exact
  cached dependency at `/usr/local/bundle/gems/activesupport-8.1.3/lib/active_support/duration.rb`
  in cached image `36f828650457` (read-only, no network, 1 CPU/256 MiB/64 PIDs/30s).
  `ruby -r active_support -r active_support/core_ext/integer/time -e
  'puts ActiveSupport.version; puts 6.months.seconds'` printed **8.1.3** and
  **15778476**. Duration is 182 days 14:54:36, not calendar months or 168 days.
  This dependency check is not Mastodon image/peer evidence.
- Rollout has **empty historical activity**, with no timestamp backfill. A fresh
  bootstrap owner is a new activation. Public instance/NodeInfo values remain
  unchanged. Future aggregation must use `D-28 <= day < D` / `D-168 <= day < D`
  and exclude expired buckets before physical cleanup. SQL fixture queries prove
  that storage contract, not a cached/public aggregation implementation.

### Actual bounded evidence

- NAS workspace `/srv/workspaces/rustodon-daily-activity-16c751c-alice`, image
  `7203e0222e2bb72e0b83ab623051873604874cc41a688e318b84691bfa77ad8a`;
  4 CPUs/6 GiB/512 PIDs, 870s container + 900s outer timeout/15s kill,
  read-only source/root and a bounded temporary filesystem. Shared cargo caches
  reused, not copied/pruned. Dedicated internal network, no published ports.
  Task-owned PostgreSQL **14.23** image
  `1a6c2409ab71f4d054d676ba09d9b74b5d843d805bd5a0cf08314d27ca659d37`;
  1 CPU/512 MiB/128 PIDs/14400s hard lifetime. All heavy invocations sequential.
  No production, environment/credential/backup synchronization or deployment.
- TDD RED migration prefix `[1..5] != [1..6]` (`red-plan.log`) → GREEN.
  Bootstrap initially rejected newly recorded activity as unexpected work-table
  rows → exact initial-activation baseline and rerun/drift tests GREEN.
- `--all-features --lib activity:: -- --ignored --nocapture --test-threads=1`:
  **4 passed** (`activity-green.log`, 14.61s). Eligibility/locality, first
  transition simulation, unconfirmed creation, confirmation, failed/partial
  authentication, complete password/TOTP/backup, final password fence,
  membership-failure session rollback, retry/daily uniqueness, nil/strict 24h
  concurrent claims, actual-clock TTL refresh, expired-generation removal,
  historical disable/suspend/delete, restricted SQLSTATE 42501 and future union
  boundaries. A repeated fixture attempt initially reused a confirmation token;
  test tokens are now unique and the final run used a fresh restore.
- `--test media_state local_upload_http::local_upload_browser_media_access`:
  **1 passed** (`media-final.log`, 29.54s) on a fresh restored fixture with actual
  runtime/writer pools. Existing GET/HEAD media checks now snapshot user sign-in
  fields plus both activity tables, with a deliberately due retained owner.
  Subsequent real HTTP checks cover untracked ordinary bearer reads and tracked
  credentials/HTML requests, daily dedup/throttle and unchanged sign-in count.
  This is not a browser gate or the full media lane.
- `--test standalone_bootstrap standalone_bootstrap_installs_and_verifies_exact_baseline`:
  **1 passed** (`bootstrap-final.log`, 39.19s) on a separate fresh empty database
  with distinct non-superuser installer/runtime/writer roles and task-owned
  temporary media. Includes real HTTP smoke, exact roles, first activation,
  verification-only expiry preservation and extra-member/future-expiry rejection.
- `--test operational_schema instance_activity_upgrade_from_five_preserves_history_and_grants`:
  **1 passed** (`upgrade-final.log`, 18.41s): exact v5→v6, no backfill/user changes,
  rerun, runtime SELECT/no writes, writer CRUD and exact writer/reader validation,
  catalog drift rejection. Fresh operational lifecycle **1 passed** earlier
  (`schema-lifecycle.log`, 105.59s); final fresh migration setup also passed.
- Focused pinned activity source contract **1 passed** (`source-final.log`).
  Ordinary all-feature library **356 passed / 26 ignored** (`ordinary-lib-green.log`);
  migration plan **1 passed** (`plan-green.log`). Existing upload schema upgrade
  **1 passed** on its separate owner-only fresh fixture
  (`upload-schema-isolated.log`); initial use of the restricted fixture correctly
  rejected an unconfigured known-writer ACL, not a migration defect.
- Strict Clippy on library plus changed integration targets **passed**
  (`lint-final.log`). Full all-target Clippy is **not green**: existing untouched
  test-helper lints in paperclip/worker/media_processor remain; the new test's
  unreadable literal was fixed. Aggregate offline harness passed peer/image/
  selector shell checks, then stopped because the tools image has no Node.
  Standalone Python harness could not start because it also lacks Python.
  No full matrix, browser, differential, peer or release claim.
- Logs, scripts, source hashes, direct catalog extracts and dependency evidence
  retained under ignored `target/daily-activity-evidence/` and the task workspace.
  Final local/worker SHA256 comparison **22/22 matched**; formatting and diff
  whitespace checks passed. Removed only the task PG container/anonymous volume
  and internal network; no task containers remain. Shared caches/workspaces were
  retained. Parent independent review pending; no commit or push.

## 2026-09-19 — Daily activity storage/auth slice scoped, not implemented

- Created/indexed `record-transactional-daily-instance-activity` beneath the
  instance metrics parent, with the requested storage, retention, auth/media,
  least-privilege, rollout and bounded verification requirements.
- Verified clean pinned Mastodon HEAD `1440d55b139e39ec722c2a3db7f60b66cd889048`
  and operational current version 5 on base `16c751c`. Recorded candidate
  bucket/member design and exact write-expiry/window source references in issue.
- Traced separate credential-verification and fenced session-creation
  transactions: activity belongs after the final fence in the session
  transaction, not at the earlier committed success audit. Main frontend reads
  sessions without the settings/session touch hook; media intentionally remains
  mutation-free. No auth refactor or implementation undertaken.
- Subagent delegation unavailable (`nesting limit reached (depth 1 of 1)`);
  parent independent review remains outstanding. No TDD or NAS/PG14 fixture
  gates run, no full-matrix claim, no production access, no commit.

## 2026-09-19 — Reviewed bounded hashtag acceptance closure (75f770)

- Parent review 75f770 approves the bounded harness commit without blockers.
  Preserve strict Rails differential **FAIL 54/59**, all 59 statuses/equivalent
  rejection semantics. Five text differences accepted as nonblocking ordinary
  compatibility pending an actual client dependency; no normalization/fake PASS.
- Archive exactly the bounded browser/differential acceptance child, profile-read
  prerequisite (c493, bd01aca plus successful browser continuation), and local
  API child (8e22f52 review 2 plus recorded API/grant/stream/source gates). API
  closure does not claim browser coverage. Historical evidence remains intact.
- Keep support-hashtag-and-featured-tag-controls OPEN. Created/indexed focused
  metadata/typed-name editor follow-up: captured empty editor says maximum tags;
  suggestion Add/Delete passed, typed entry did not. Verified source has frontend
  zero fallback and existing Rust max_featured_tags literals; cause/fix not yet
  established. No implementation, new browser or differential attempt this turn.
- Peer AddHashtag/RemoveHashtag and Redis-history semantic boundaries unchanged.
  No production, push or application changes. Raw evidence stays ignored in
  target; explicitly stage intended harness/docs only, excluding __pycache__.
- Pre-commit checks passed: focused offline 4 + existing browser regressions 8,
  cargo fmt --all -- --check, diff whitespace, exact issue-index closure set and
  unchanged raw-evidence checksums. No extra broad fixture/test gates.

## 2026-09-19 — bd01aca hashtag browser PASS; real Rails differential findings

- Resumed indexed acceptance on exact bd01acae2bc4e1b8a75bd95648e216535c790330,
  reviewed prerequisite c493. Verified clean read-only Mastodon 1440d55b; exact
  git archive and pinned assets. No app code, production, deployment, push/commit.
- Final fresh uninterrupted real-click browser PASS: tag header/history, four
  header controls, reload state, representative home inclusion/removal, profile
  featured-tag add/Delete, reload and public-profile presence/removal. No API
  replacement or profile PATCH. Initial 1-GiB request interruption unproved;
  2-GiB run reached a DOM-valued wait; boolean correction passed third fresh run.
- Serial real pinned Rails HTTP differential ran twice. Final 59 cases: 54 strict
  matches, all statuses match, count "3"/date and final limit 10 match. Covered
  supported local methods, normalization/duplicates, ownership/auth scopes and
  limits. Five error strings differ: invalid lookup; missing/empty/invalid name;
  Rails header-limit message repeated three times vs once. Strict FAIL retained;
  no application fix or silent error normalization. Parent disposition required.
- First differential's four additional date differences came from rewriting an
  older-ID timestamp; corrected only the seed to a new chronological highest-ID
  current status. Second/final attempt removes these date differences. No third
  attempt, application timestamp change or upstream incidental-defect emulation.
- Differential reuses mastodon-fixture pinned constants/start_differential_web,
  with no host ports and explicit bounds. Separate restored Rust/Rails PG14.23
  DB/media, restricted runtime/writer and Rails non-superuser DB owner. Internal
  task networks; no Sidekiq/Rust workers or federation delivery. History actual
  Rails 0/0 vs Rust 1/1 retained, excluded from equality as DB/Redis semantics;
  IDs bijectively paired and only equal-count ordering ties normalized.
- Bounded source builds 4 CPU/6 GiB/512 PIDs/870s; app and Rails each 2 CPU/2 GiB;
  full invocations 1500s. Other explicit process/memory/time limits in README.
  Task containers/PG volumes/networks/media/keys/env/sessions/build output removed
  and absence checked. Final container states running/non-OOM before teardown.
- New offline guards RED -> GREEN; final 4 focused + 8 existing browser regressions
  pass, JS/shell syntax and diff checks pass. Full mise harness aggregate stops
  in existing peer test on macOS BSD stat -c; not a full aggregate pass. No
  unrelated portability work. Disabled-user probe scope-confounded; no separate
  suspended-account precedence or anonymous public-profile browser proof.
- Sanitized evidence/hashes: target/hashtag-acceptance-bd01aca; NAS originals in
  rustodon-hashtag-{browser,differential}-bd01aca-alice, earlier attempts separate.
  Executed application/helpers match; final generator reproduces runtime scripts.
  Parent harness review pending. Issues remain open for five string dispositions;
  peer AddHashtag/RemoveHashtag and Redis history equivalence remain deferred.

## 2026-09-19 — Hashtag browser acceptance blocked on profile read (8e22f52)

- Created/indexed `accept-local-hashtag-controls-browser-differential` before
  harness work. Reused fixed remote-browser adapter, exact source archive,
  verified clean read-only Mastodon 1440d55b, pinned frontend assets, NAS tools
  7203/browser d633/PG14.23. No app edits, production, deployment, push or commit.
- Offline adapter missing-helper RED -> 1 passed. Two fresh bounded serial
  browser attempts: first failed a harness link-vs-button selector; corrected
  once. Second real-click controller passed header/nonzero history, Follow and
  Unfollow, reload state, representative home inclusion/exclusion, Feature and
  Unfeature, reload and public-profile presence/removal. Not a full gate pass.
- True ordinary UI blocker: profile editor GET /api/v1/profile returns 404.
  Suggestion click POST /api/v1/featured_tags returns 200, but the missing profile
  model prevents rendering the item/Delete control even after reload. Pinned
  apiGetProfile and profile_edit reducer confirm dependency. No API workaround,
  no app fix; paused for scope decision. Profile removal and subsequent browser
  suffix are unexecuted and need revalidation after the profile read is supplied.
- Exact Rails image is cached, but differential was NOT run and no selector or
  Rails DB/media clone created before pause. Normalization/duplicates/limits/
  owner IDs/scopes parity remains unproved. Current-DB history versus Redis
  retention/activity is intentionally excluded from future incidental parity,
  never normalized to zero. Existing WebSocket test not rerun; peers deferred.
- Internal task network, no host ports or workers/peer routes; explicit resource
  and wall bounds, restricted runtime/writer proof. Task containers, PG volume,
  network, media, TLS keys/env/sessions and build output removed/absence checked.
  Sanitized actual-call/DOM/screenshot evidence exported to ignored
  target/hashtag-browser-acceptance-evidence; executed harness hashes match.
  Credential scan only matched static asset `password-ByJdIm8f.png: OK`.
- Nested delegation denied; independent parent harness review pending. Both
  issues remain open; README records exact bounds, partial evidence and gaps.

## 2026-09-19 — Hashtag review 1 ce0d follow-up

- Addressed the two directly relevant medium findings (review reported no highs):
  relationship INSERT/sequence-USAGE grants now stay inside the existing grant
  transaction; pre-rate-limit follow checks use a lightweight normalized EXISTS
  query rather than aggregating history before and after mutation.
- Readback policy is explicit: failed bounded history returns HTTP 500, never
  fabricated zero history; the committed idempotent relationship remains safe
  to retry. Actual table-lock timeout regression RED -> GREEN proves this.
  Grant placement regression also RED -> GREEN.
- Expanded existing restricted-role HTTP fixture with seven UTC day/boundary,
  distinct-author, public-only, suspended/silenced, boost/deleted and future
  exclusion assertions. No storage/cache/job/protocol expansion.
- Bounded NAS PG14: focused HTTP/grant tests 3/3, reused API-driven home/WebSocket
  fixture 1/1, verified read-only pinned source contracts 9/9, strict all-feature
  library Clippy/fmt/diff checks passed. Did not chase stress SIGKILL or unrelated
  lint. Evidence uses review2-* logs in target/hashtag-controls-evidence/ and NAS
  task workspace; source hashes verified; task PG/volume/network removed.
- All changes uncommitted, no production push/access. Ready for parent independent
  review 2; browser/peer/differential/history-retention boundaries unchanged.

## 2026-09-19 — Bounded local hashtag controls (d9f8ae1)

- Created/indexed `implement-local-hashtag-control-apis.md` before code; parent
  remains open. Added lookup/follow/unfollow/feature/unfeature and featured
  collection create/delete without schema, jobs, protocol or browser changes.
- Reused account locks, transactions, serializers, dynamic home/stream recipients.
  Pinned normalization/duplicates, owner deletion, concurrent limit 10, narrow
  writer INSERT/sequence USAGE grants and preflight checks.
- Header history: seven-UTC-day public/non-boost aggregate with two-second DB
  timeout; excludes deleted/suspended/silenced data. Not Redis retained counts,
  registration-time history or trend equivalence; other history projections
  unchanged. AP AddHashtag/RemoveHashtag and historical stream cleanup deferred.
- Route RED -> GREEN; HTTP lifecycle found missing local-delete count decrement,
  now reusing the existing helper. Stream fixture uses real API follows/unfollows
  and restricted roles; stale silenced delete assertion now matches existing
  cleanup semantics, without changing the streaming engine.
- Sequential bounded NAS PG14: default lib 343 passed/7 ignored; all-feature lib
  354 passed/21 ignored; restricted-role HTTP 1/1 (including privilege revocation,
  concurrent limits/follows, counts/history); WebSocket/home lifecycle 1/1;
  pinned source contracts 9/9. Exact clean 1440d55b source verified on host and
  mounted read-only. Wrapper could not verify inside git-less tool image; host
  verification and actual source tests recorded separately, not differential.
- All-target/all-feature cargo check, all-feature lib strict Clippy, fmt/diff
  checks passed. Full strict Clippy blocked by existing media_processor,
  paperclip test helper and worker/local_uploads test lints, not changed here.
  Two-request concurrency passed; six-request stress SIGKILLed within 6-GiB
  container. No stress-success, Rails HTTP differential, browser, peer or full
  milestone gate claim. Child nesting limit prevents independent review; parent
  review required. All changes deliberately uncommitted; issues remain open.
- NAS task workspace rustodon-hashtag-d9f8ae1-alice, tools 7203e0222e2b,
  PG14.23 1a6c2409ab71; runner 4 CPUs/6 GiB/512 PIDs/870s (outer 900s+15s),
  PG 1 CPU/512 MiB/128 PIDs/7200s; internal network, no published ports.
  Only intended source/explicit new helpers/tests synchronized; final hashes
  matched. Task PG/anonymous volume/network removed; shared caches and clean
  source untouched. Evidence in ignored target/hashtag-controls-evidence/ and
  task NAS workspace. No production access.

## 2026-09-19 — Status-search reconciliation after 0994711

- Docs-only archive of four status-search issues on parent approval. Accepted
  reviews: `knownlookupreview2collision`, `uncachedreview2policy`,
  `browserreview8e75`; no remaining high/minimum-criterion blocker reported.
  Current issue dispositions supersede stale pending/uncommitted statements;
  historical hashes, timings, red/green notes and original bytes preserved.
- Canonical exact-URL minimum without ES accepted, including final uninterrupted
  real-input Posts/exact navigation/reload, cached additional fetches 0, signed
  TLS public GET 1, denied empty and no private persistence/mention evidence.
  Coordinator reruns: local 4 pass / 1 pinned-source skip; Linux pinned-source
  5 pass. Not rerun by this docs-only reconciliation.
- Pinned URL-branch account/text filters do not apply to exact lookup; following
  belongs to account search. Stricter mute/block suppression is intentional.
  HTML alternates, unredirected mismatched display URLs, actor-URL dispatch and
  full-text remain deferred. All-tab empty and exactly-one public-fetch checks
  were observed, not hard assertions: nonblocking reviewer follow-ups only.
- Only four index links moved; issue details retained. Full matrix pending.
  User's entire batch remains open: hashtags/activity next, then combined
  verification. No code, production, deployment or push. TDD inapplicable to
  docs-only reconciliation. Index/link/history-preservation and diff checks pass.
  Fresh docs review delegation hit the session nesting limit; no new independent
  docs review claimed. Accepted parent implementation reviews remain as above.

## 2026-09-19 — Bounded status-search browser acceptance (exact c3497f5)

- Indexed `accept-status-search-browser` before new harness files. Reused the
  existing NAS remote-browser lifecycle with a fixed task-only adapter and
  separate TLS Note source/passive XHR observer/controller; no application or
  existing media-acceptance files changed. No deployment, push or commit.
- Verified read-only reference clean at 1440d55b139e39ec722c2a3db7f60b66cd889048;
  exact git archive c3497f5cdf71678c70542689e079ac589de253a1, pinned frontend packs,
  NAS tools 7203e0222e2b/browser d6337b96fb60/PG14.23 1a6c2409ab71. Migration 5,
  narrow runtime/writer, owner only for setup/grants/migration/observations.
- Final fresh uninterrupted controller PASS: signed-in native URL input + Enter,
  Posts click, visible known-local and canonical uncached public Note results,
  timestamp click to exact returned status permalink, independent frontend
  status GET/render after reload. Actual search XHRs: resolve=true, limit=11,
  omitted type and type=statuses, offset absent. No response injection.
- Controlled task CA/TLS source cryptographically verifies instance-actor RSA
  signatures. One public GET; cached repeat and known-hidden search add zero
  remote source fetches. Source counts all paths except explicit readiness.
  Known-hidden and unrelated private Note display No results/no status DOM;
  private count 0, public count 1, task mentions 0, total mentions unchanged (8),
  known-hidden row hash unchanged. Uncached baseline count 0. Existing Bob actor
  reused; this does not exercise unknown-actor discovery or a real peer.
- First attempt passed known URL but failed the source verifier because the
  harness expected Alice rather than the instance actor key; fixed with a RED
  regression guard. Second passed; final fresh run additionally checked complete
  source request counts and before/after privacy state. Attempts retained apart.
  Helper missing-file RED -> GREEN; final helper/crypto/opt-in pinned query
  contracts 5 passed; unchanged media harness tests 8 passed; diff check passed.
- Reused explicit serial bounds: build 4CPU/6GiB/512PID/900s, PG1CPU/512MiB,
  web2CPU/1GiB, browser2CPU/2GiB, source/TLS1CPU/256MiB, forward1CPU/128MiB;
  runtime PID/wall limits, controller600s, total1500s. No published ports or
  production resources. Closed browser and removed owned containers/PG volume,
  network, fixture state/media, certificates/keys/env and fresh build output.
- Final sanitized evidence and SHA256SUMS under ignored
  `target/status-search-browser-evidence/evidence/`, original NAS workspace
  `/srv/workspaces/rustodon-status-search-browser-c3497f5-alice`. Executed harness
  and representative application hashes match checkout. No raw session/HAR/token
  data exported; credential-pattern scan and exported checksums passed.
- Boundary: singleton has no load-more UI. Pinned source contract checks expand
  limit/offset and Rails URL typed-offset behavior; nonzero-offset browser gate
  NOT run. Full-text, HTML alternates, actor URLs, hashtags excluded. Prior HTTP
  security suite not rerun; full Rust pinned-source/browser/DB/worker/peer/
  differential/check/Clippy lanes not run. No app bug encountered.
- Independent parent review remains pending: task delegation rejected at nested
  depth limit. Leave subissue open and everything uncommitted. Existing DEVLOG
  trailing NUL bytes were already in HEAD and preserved, not repaired in scope.

## 2026-09-19 — Uncached search review 1 test-isolation follow-up

- Parent review `a30e` reported no blockers/high findings and one directly
  security-relevant medium fixture gap. Closed that gap with tests only:
  table-driven independent viewer-block, reverse-block, mute, suspended-author
  and viewer-domain-block cases in the existing uncached fixture.
- Each case installs exactly one policy, checks cached denial with zero fetches,
  checks a unique uncached URL returns empty with no persisted status, then
  removes exactly its policy and verifies the cached positive control again.
  Author policies require exactly one signed object GET; domain denial requires
  zero. The prior viewer block no longer masks the domain case.
- No behavior/source-service changes. Production source SHA-256 values match
  pre-review evidence. Fixed test-only Clippy naming/import-placement findings.
  Test-only follow-up: no new production red/green cycle claimed. First expanded
  run failed because the cleanup positive control compared old account counters
  captured before later imports; corrected it to capture the current projection
  before the matrix. The policy matrix then passed against unchanged production.
- Final restricted runtime/writer Linux PG14 command (`review1-verified.log`):
  `cargo test --locked --offline --features test-support --lib web::account_search_tests:: -- --include-ignored --nocapture --test-threads=1`
  **4 passed**, 0 failed, 368 filtered, 28.23s. Fresh disposable restore; owner
  credentials only for setup/policy fixtures, actual HTTP uses restricted roles.
- Final library Clippy with `--features test-support --lib -- -D warnings`
  passed (`review1-clippy-verified.log`). Broader `--lib --tests` still fails on
  pre-existing unrelated diagnostics in tests/media_processor.rs, src/paperclip.rs
  and src/worker/local_uploads/tests.rs; no diagnostics remain in changed search
  code/tests (`review1-test-clippy-verified.log`). Local fmt/diff checks passed.
- Reused task workspace `/srv/workspaces/rustodon-uncached-search-54dc5c9-alice/`
  and explicit test-file-only sync. Same NAS7203 tools image and bounded
  4 CPU/6 GiB/512 PID/870s +900s outer limits. Fresh PG14 resources used
  1 CPU/512 MiB/128 PIDs/1800s, task internal network, no host ports. Verified
  final source hashes against NAS (`review1-source-sha256.txt`). Removed both
  runs' task PG anonymous volumes, PG/runner containers and network; absence
  verified (`review1-cleanup.txt`). No production access, broad prune or shared
  cache removal. Logs collected under ignored `target/uncached-search-evidence/`.
- Leave uncommitted for parent review 2 (maximum two substantive rounds).
  HTML/actor-URL behavior unchanged; browser work remains next, not run here.

## 2026-09-19 — Uncached exact status URL search (54dc5c9, uncommitted)

- Created/indexed `meta/issues/search-uncached-exact-status-urls.md` before source
  edits. Changes remain uncommitted; parent and subissue remain open for review.
- Verified the read-only reference checkout is clean at exactly
  `1440d55b139e39ec722c2a3db7f60b66cd889048`; inspected SearchService and
  ResolveURLService. No claim of full URLService compatibility or source-contract
  gate execution in this slice.
- Added narrow `status_resolution` service: existing signed RemoteFetcher,
  actor resolver/WebFinger, normal Note/Question parser and
  `apply_remote_note_create(..., None, ...)`. No worker internals exported,
  schema/grants/job types/source identity scheme changed. Missing-parent work
  uses the existing durable outbox resolver job; its execution was not tested.
- Lookup now distinguishes Unknown/Denied/Found before any fetch. Authoritative
  identity tiers and ambiguous-match denial are preserved. Existing audience
  policy is checked before fetch and again during final authorized projection.
  Ingestion parser-backed preflight rejects invalid documents, tombstones,
  blocked/muted/suspended known authors and unauthorized audiences before actor
  creation. The searching user is NEVER an inbox delivery target; actual to/cc
  and Mention data, or an existing follow, must supply private access.
- Exact URL coverage: cached URI/display URL/local aliases remain supported;
  uncached canonical HTTP(S) Note/Question IDs and same-origin transport redirects
  to an exact canonical response ID are supported. Arbitrarily advertised IDs,
  cross-origin redirects and attribution are rejected. Existing URL-aware domain
  canonicalization preserves non-default ports. There is no trusted HTML Link
  discovery helper in RemoteFetcher, so HTML and an unredirected display URL
  returning a different canonical ID are explicitly unsupported. No crawler,
  Create/Announce wrapper expansion, collections or full-text search was added.
  Existing account-handle resolution remains covered; actor-URL dispatch is not
  newly implemented by this status-only resolver.
- TDD actual RED (`red.log`): newly added restricted-role fixture failed at
  `uncached public URL`, expected 1 result, actual 0, before production edits.
  Initial harness attempts failed on startup readiness/missing disposable roles;
  these were corrected with explicit owner-only setup, not credential fallback.
  Intermediate fixture corrections included the actor context, PKCS#1 fixture
  key format and checking the actual durable outbox payload, not undispatched
  durable_jobs. None are claimed as behavioral red/green evidence.
- Final GREEN (`canonical.log`), fresh task-owned PG14 restore:
  `cargo test --locked --offline --features test-support --lib web::account_search_tests:: -- --include-ignored --nocapture --test-threads=1`
  **4 passed**, 0 failed, 368 filtered, 27.48s. Existing account regression now
  also uses actual restricted writer credentials for HTTP resolution; owner is
  setup only. Runtime/writer grants are unchanged documented grants.
  New fixture verifies RSA-signed object/actor GETs (ordinary WebFinger unsigned),
  public/unlisted/private/direct access, private import only after a real follow,
  no fabricated mention, zero-fetch cache reuse/known-private denial/deletion/
  tombstone, early anonymous/app/limit/type/offset/origin rejection, mismatched
  IDs/attribution, malformed/unsupported/HTML/404/cross-origin responses, denied
  author and domain, Question and normal durable missing-parent outbox work.
  It checks no new actor rows for early rejected remote documents.
- `cargo clippy --locked --offline --features test-support --lib -- -D warnings`
  passed on final source (`canonical-clippy.log`). Ordinary remote tests: **39
  passed, 1 ignored** (`remote-unit.log`); web tests: **91 passed, 2 ignored**
  (`web-unit.log`), before the final URL-domain canonicalizer substitution and
  added tombstone/private-deletion assertions. These are not a live HTTPS/peer
  gate. Local fmt/diff checks passed; no browser, full matrix or release claim.
- NAS workspace `/srv/workspaces/rustodon-uncached-search-54dc5c9-alice/`;
  tools image `7203e0222e2bb72e0b83ab623051873604874cc41a688e318b84691bfa77ad8a`,
  4 CPUs/6 GiB/512 PIDs/870s, outer 900s +15s kill. PG14 image
  `1a6c2409ab71f4d054d676ba09d9b74b5d843d805bd5a0cf08314d27ca659d37`,
  1 CPU/512 MiB/128 PIDs/5400s initially, 1800s final rerun; isolated internal
  task network, no published ports. Only tracked HEAD source and explicit source
  edits/new helper were synchronized. No production access. Task-specific PG,
  anonymous volumes, runner and network removed; evidence/scripts retained in
  ignored `target/uncached-search-evidence/` and NAS workspace. Shared build
  caches were reused, never removed. Final source hashes verified against NAS.
- Independent review is pending with the parent. This child could not spawn a
  reviewer (configured nesting depth limit); no independent review is claimed.

## 2026-09-19 — Exact status URL identity collision review fix

- Resumed after unlock; first NAS access succeeded. Loaded nas-podman and
  repo-issues skills. Parent independent review `6a18159b` identified a high:
  OR + lowest-ID selection let foreign display URLs impersonate authoritative
  local aliases/canonical URIs, or suppress genuine matches when hidden.
- Bounded fix only in repository lookup: a CTE ranks validated local aliases
  before exact canonical URI before display URL. It selects only a unique
  best-tier identity, then performs existing search suppression/normal audience
  projection. Denied authoritative rows cannot trigger lower-tier fallback.
  Ambiguous display-only matches fail closed before access filtering. No schema,
  ingestion, resolver, authentication architecture, or privilege changes.
- TDD persisted RED before repository edit (`collision-red.log`): exact
  `web::account_search_tests::v2_known_status_urls_use_authorized_projection`
  failed with older foreign IDs, hidden colliding rows suppressing genuine
  targets, denied authoritative targets returning foreign content, local alias
  versus foreign canonical URI, and ambiguous display URL substitution.
  Command: `cargo test --locked --offline --features test-support --lib web::account_search_tests::v2_known_status_urls_use_authorized_projection -- --ignored --exact --nocapture --test-threads=1`.
- Final GREEN (`collision-final.log`), fresh disposable PG14 restore:
  `cargo test --locked --offline --features test-support --lib web::account_search_tests:: -- --include-ignored --nocapture --test-threads=1`
  **3 passed**, 0 failed, 368 filtered, 19.42s. Same named pure/account/status
  tests as prior evidence; status test now includes collision matrix. Existing
  fixture setup uses owner credentials, HTTP uses actual restricted runtime
  credentials, and SQLSTATE 42501 status-update denial remains asserted.
  No claim that a separate restricted writer mutation gate ran.
- `cargo clippy --locked --offline --features test-support --lib -- -D warnings`
  passed (`collision-clippy-final.log`).
  `cargo test --locked --offline --features test-support --lib web::tests:: -- --test-threads=1`
  passed **91**, ignored **2** (`collision-web-unit.log`); subsequent edit only
  replaced a redundant closure in the status test, then reran the focused gate.
  `cargo fmt --all --check` and `git diff --check` passed.
- Broader `cargo clippy --locked --offline --features test-support --lib --tests -- -D warnings`
  still fails on existing unrelated warnings in tests/media_processor.rs,
  tests/mastodon_schema.rs, src/paperclip.rs and src/worker/local_uploads/tests.rs
  (`collision-clippy-tests-final.log`). Fixed our earlier redundant closure;
  no diagnostics remain in the changed search files. Did not expand this fix.
- Verified SHA-256 main checkout vs final NAS source with `shasum -a 256 -c`:
  src/web.rs: 5a0175cf5b5bbf081bc088d3eb3057ca0dd2281a9b6de82898f202b9b31dd8e1
  src/mastodon/repository.rs: 2ff01ef497d8dc8d58811977a6488f34cf96a73c53157557d9d7c1e5dc0216c2
  src/web/account_search_tests.rs: 862d8a774fd38b33282cad83d7dfa12041a5a3916c6ef410c6355f1867e2f638
  Only explicit changed source files were synchronized. No production data,
  credentials, .git or build output was copied into the test workspace.
- Resource lifecycle: inspected timed-out `status-search-pg-alice`, removed it
  with its verified anonymous volume
  `9f38bf4b3abedf06b2ee337fdf91b26c477904b407aca837d6493845d85ca0da`.
  Recreated only this PG container on existing task `status-search-alice`
  network, database `uploads`, pinned PG14 image and same bounds (1 CPU,
  512 MiB, 128 PIDs, 5400s, no host ports; disposable fsync/synchronous_commit off).
  Tools retained 4 CPU/6 GiB/512 PIDs/870s +900s outer/15s kill bounds and image
  7203e0222e2bb72e0b83ab623051873604874cc41a688e318b84691bfa77ad8a.
- Cleanup completed after final tests: inspected actual mounts and network
  membership, stopped/removed `status-search-pg-alice` with new anonymous volume
  `0f9fa57d804d4c0f730ca91803e3f9f9f66a5c903217572b6b00927ef0ef0150`, then
  removed `status-search-alice`. Verified both recorded volumes, PG container,
  auto-removed `status-search-test-alice` runner and network absent. Evidence:
  `collision-cleanup.txt`, before-cleanup inspect JSONs. No broad pruning.
- Retained NAS workspace `/srv/workspaces/rustodon-status-search-f93b6ef-alice/`
  with source/, evidence/, run.sh and reset.sh. Collected all existing/new logs
  and scripts to ignored local `target/status-search-evidence/nas/`.
  Shared `/srv/workspaces/rustodon-null-route-20260918/{cargo-home,source/target}`
  caches were reused but never removed. This supersedes prior SSH/cleanup gaps.
- Leave all changes uncommitted and both issues open for parent review 2.
  Uncached resolution/browser/full matrix remain deferred; no push/deployment.

## 2026-09-18 — Known exact status URL search (f93b6ef, uncommitted)

- Created/indexed `meta/issues/search-known-exact-status-urls.md` before source edits.
  Parent remains open; uncached resolution, browser regression, Elasticsearch,
  new schema/protocol/jobs/privileges and full fixture matrix are excluded.
- Verified actual reference checkout root using
  `git -C target/mastodon-v4.6.5 rev-parse --show-toplevel HEAD`:
  `/Users/lainsoykaf/repos/rustodon/target/mastodon-v4.6.5`, revision
  `1440d55b139e39ec722c2a3db7f60b66cd889048`; `status --porcelain` empty.
  Inspected `app/services/search_service.rb` directly, without modifying it.
- URL branch requires resolve=true and authenticated read/read:search user,
  is exclusive, accepts absent/blank/statuses type, returns none at limit=0,
  suppresses positive offset for specified type but ignores it for blank type.
  Text account_id/min_id/max_id/following filters do not apply to URL lookup,
  as in the reference. Anonymous resolve/pagination now returns 401, including
  account resolution; existing account tests updated for this explicit contract.
- Read-only repository lookup accepts exact stored uri/url or configured-origin
  local permalink/username/numeric AP object paths, checks username/account ID,
  and handles nullable local flags consistently with the normal projection.
  Existing StatusAccess/authorized_status remains the audience/deleted/suspended
  authority. Additional viewer block/mute/account-domain-block suppression is an
  intentional safer search policy, not a claim of bug-for-bug context silencing.
  No remote request or status creation path was added.
- TDD: NAS pure contract RED failed for the missing helper; GREEN passed.
  Persisted baseline-handler replay (new pure helper retained for compilation)
  failed as expected: empty statuses versus an ordinary visible status projection.
  Baseline replay log: `baseline-http-red.log`. The persisted test was added
  after initial implementation, so this behavioral replay is retrospective RED.
- Final NAS command (log `http-verified.log`):
  `cargo test --locked --offline --features test-support --lib web::account_search_tests:: -- --include-ignored --nocapture --test-threads=1`
  **3 passed**, 0 failed, 368 filtered; test execution 19.56s. Named tests:
  `known_status_url_branch_contract`,
  `v2_accounts_reuse_search_and_authenticated_resolution`,
  `v2_known_status_urls_use_authorized_projection`.
  Covers local permalink/AP aliases, persisted remote uri/url, rich fixture
  media/poll/quote projections compared with ordinary GET, wrong origin/author,
  unknown URLs, deleted/suspended, private/direct denial and mention membership,
  blocks/mutes/domain blocks, type/offset/limit, URL-filter independence,
  anonymous/application/scope rejection and existing account resolution behavior.
  HTTP status lookup has no configured writer; runtime UPDATE denial asserts
  SQLSTATE 42501. Owner connection is used only for disposable fixture setup.
- `cargo clippy --locked --offline --features test-support --lib -- -D warnings`
  passed (`clippy-verified.log`). Earlier `--lib --tests` exposed unrelated
  existing warnings in `paperclip.rs` (needless_pass_by_value) and
  `worker/local_uploads/tests.rs` (items_after_statements); not fixed here.
  Local cargo tests blocked by rustc 1.97.0 versus required 1.97.1; NAS used the
  pinned Linux tool image instead. Local formatting and diff whitespace checked.
- Disposable NAS workspace `/srv/workspaces/rustodon-status-search-f93b6ef-alice`;
  source sync was tracked files only plus exact edited source, not environments,
  credentials, .git or output. Tools image
  `7203e0222e2bb72e0b83ab623051873604874cc41a688e318b84691bfa77ad8a`,
  sequential 4 CPU/6 GiB/512 PIDs/870s container +900s outer/15s kill,
  read-only source/root, dropped caps, task tmpfs and reused dependency/build
  caches. Task PG14 image
  `1a6c2409ab71f4d054d676ba09d9b74b5d843d805bd5a0cf08314d27ca659d37`,
  1 CPU/512 MiB/128 PIDs/5400s; internal network, no host ports, disposable DB
  with fsync/synchronous_commit disabled. Fresh restores between fixture runs.
  Runtime grants reuse bootstrap contract and revoke PUBLIC DB/schema/function
  privileges; an initial harness writer-grant mismatch was corrected, without
  changing production schema/grants. Logs/scripts remain in remote evidence/;
  local runner/reset/baseline replay source in ignored target/status-search-evidence/.
- Gaps: independent task review rejected by nesting limit (1/1), twice including
  explicit read-only review attempt. No independent review claim. SSH agent
  locked after final successful focused gates, preventing extra web-unit/test
  Clippy invocation, log collection and task cleanup. Asked user to unlock; no
  workaround attempted. Task PG container `status-search-pg-alice` is time-bound
  but container/volume and network `status-search-alice` still need task-only
  cleanup after unlock. No production touched, no commits, issues remain open.

## 2026-09-18 — reviewed rich-media closure reconciliation

- Archived remote policy/transport, worker, representations and browser slices,
  remote caching parent and overall frontend media parent after checking combined
  acceptance and parent-confirmed review 2. No unmet acceptance blocker remains;
  local milestones were already archived.
- Completion overrides supersede historical uncommitted/review-pending notes:
  implementation `be6c7cd`, `41a771e`, `8b2f49e`; harness `fe6e461`. Final browser
  evidence is the fresh uninterrupted exact-`8b2f49e` run (1,208 requests, zero
  origin media); parent offline contracts 8/8. Detailed evidence references remain.
- Docs-only: no gate reruns, code/production changes or push. Actual transport/
  power loss, full fixture/release matrices and peer gates remain unclaimed.
  Earlier handoff statements below are historical, not outstanding review work.

## 2026-09-18 — bounded remote browser acceptance, review 1 corrections

- Exact application `8b2f49e` ran in fresh disposable PG14 with restricted roles,
  pinned frontend/source and debug/test-support-only task TLS. Real signed ingress,
  importer enqueue, held/released Pull worker and native WebSocket convergence
  passed for MP4, audio and AVIF. No application edits or production work.
- Reload checks now use actual frontend XHR attachment responses (audio preview
  remains null with no small metadata). Per-stage network resets cannot borrow
  prior live positives; sanitized aggregate zero-origin evidence is retained and
  each dedicated reload separately checks DOM hotlinks. Eight offline regressions
  pass. Executable bounded launch/readiness/cleanup recipe replaces prose-only setup.
- Initial launcher navigation failed before the controller. The one permitted
  harness correction added verified end-to-end TLS readiness. The subsequent
  fresh controller passed uninterrupted: PNG poster before MP4 playback (0->2
  decoded frames), audio avatar fallback/playback, JPEG still decode and all reloads.
  Aggregate 1,208 requests; zero origin-media-host requests. Reload intervals:
  video 148, audio 148, still 147 requests, each with its own local-media positive.
- Evidence: `target/remote-browser-8b2f49e-r2-evidence/` and NAS workspace
  `rustodon-remote-browser-8b2f49e-r2-alice/evidence/`. All task containers, PG volume,
  network, media, certs/keys/env/session and build artifacts cleaned. Prior failed
  startup evidence kept separately. Harness/evidence uncommitted for review 2;
  not a full matrix or new private-browser/HTTP proof.

## 2026-09-18 — remote representations review 1 (uncommitted)

- M1 pinned source confirms explicit thumbnails use attachment-model locality,
  hence original `remote_url`. Kept authorization repository unchanged; aligned
  REST/AP and three existing thumbnail metadata/cleanup constructors. Generated
  MediaFile small paths unchanged. The initial thumbnail_remote_url hypothesis
  was rejected after inspecting the reference prefix interpolator.
- M2 direct remote MediaFile reads now reject explicit processing 0/1/3 using
  existing access facts. NULL/ready, explicit thumbnails and local policy remain
  unchanged; no MIME/source-suffix guessing for normalized JPEGs.
- Actual HTTP red → green covers serialized thumbnails with no remote thumbnail
  URL, GET/HEAD/range, wrong-namespace stale copies, remote stale MP4/MP3/JPEG
  states and historical NULL/local readiness. Restricted PG14 HTTP 1/1; ordinary
  library 351 passed/18 ignored; REST 18/18; pinned source 8/8. Scoped strict Clippy
  passes; broad tests use only previously documented lint exceptions. Details
  and exact logs in the representation subissue; task DB/network removed.
- One review/fix round complete. Uncommitted for parent review 2; no browser work.

## 2026-09-18 — consistent remote REST/AP/proxy representations (uncommitted)

- Created/indexed [representation subissue](meta/issues/remote-rich-media-representations.md)
  before code on `41a771e`. REST rich pending/failed previews fail closed; cached
  MP4/PNG, MP3/no-preview and normalized JPEG URLs stay local. AP original URLs
  now match installed MIME; generated small icons use the existing style contract.
- Proxy cached reads reuse streaming range/HEAD responses, existing rich-family
  caps, authorization/domain fences, cookie/bearer viewer and private caching.
  Missing rich posters never fall back to original video/audio. No ordinary cap,
  schema/job/worker, synchronous codec, or browser expansion.
- TDD red REST/AP and stale/oversize cached regressions → green. Bounded NAS7203
  evidence: REST 18/18, final library 350 passed/18 ignored, restricted PG14 HTTP
  1/1 with PNG/JPEG decoding and authorized proxy/local URL ranges/HEAD, pinned
  source contracts 8/8. Scoped strict Clippy passes; broad tests pass with recorded
  pre-existing lint allowances. Formatting/diff checks pass. Exact commands,
  failed environment attempts, hashes and limits are in the subissue.
- Task DB/network removed. No production or browser work. Nested delegation is
  unavailable; changes remain uncommitted for independent parent review (no review
  claimed), and issues remain open. Full cookie/session integration not rerun.

## 2026-09-18 — remote rich-media worker

- Reused bounded rich processing in the existing remote fetch/install lifecycle;
  normalized type/MIME and measured metadata now survive same-URL updates.
  Per-hop policy and install-time rechecks retain transport safeguards.
- Focused Linux/PostgreSQL tests under restricted runtime/writer credentials:
  8 remote-worker cases plus 1 import/same-URL regression passed, including real
  rich outputs, ambiguity/reclamation, policy/deletion, and stream convergence.
- Review-driven red/green barriers cover focus edits/clearing during fetch and
  cancellation during stream flush. The output guard stays armed until COMMIT;
  actual uncertain-commit handling remains intact. Independent review2 approved.
- Formatting/diff and all-feature focused Clippy passed; production-cfg lint
  retains the existing unrelated unused-self allowance. Detailed commands and
  evidence are in `meta/issues/cache-remote-rich-media-worker.md`. No browser,
  representation/proxy, full-matrix, or deployment completion is claimed here.

## 2026-09-18 — remote media review 1: strict processor-compatible bounds

- Fixed parent blocker `9f9122`: media-only budget is existing format input limit
  minus one; generic inclusive response limits unchanged. Corrected subissue's
  erroneous inclusive-limit claim. No worker activation or scope expansion.
- TDD boundary matrix red on exact 16 MiB image → green: fixed/chunked image,
  video and audio accept limit−1, reject exact/+1; ordinary exact still accepts.
  Direct wrapper tests prove redirect policy denial, encoding rejection, timeout
  and cancellation release of shared host budget. Transport selector **3/3, 1.77s**.
- NAS `7203e022…`, same isolated offline workspace/resources: MIME tests **4/4**,
  ordinary configured limits **1/1**, focused strict Clippy **pass**. Test-source
  Clippy with previously recorded unrelated lint exceptions **pass**. Local fmt
  and diff checks **pass**. Exact commands/bounds in the existing subissue.
- Uncommitted for parent review 2; no broader suite, production, codec or worker
  execution. Task containers removed; review workspace retained.

## 2026-09-18 — remote media policy/transport slice (uncommitted)

- Created/indexed [subissue](meta/issues/remote-media-policy-and-transport-bounds.md)
  before implementation on clean `bed710f`. Central MIME agreement uses existing
  media format/family caps; typed attachment-only fetcher requires <99 MiB audio/video,
  <16 MiB images, including explicitly missing advertisement. Fetched MIME checked
  before body streaming. Ordinary/emoji limits and security transport unchanged.
- No worker activation, serializer, schema, grant, job, lane or cache change;
  no remote feature-completion claim. Parent review 1 blocker `9f9122` corrected;
  parent review 2 pending (nested delegation unavailable); all changes intentionally uncommitted, subissue remains open.
- TDD: pure MIME missing-API red → 4/4 green; Linux transport missing-API red →
  1/1 initial green (0.45s; inclusive media boundary corrected below), with
  unchanged generic ceiling. Existing configured limits 1/1; URL/address/DNS/MIME
  checks 4/4; host-budget checks 2 passed / 1 DB test ignored.
- NAS `7203e022…`, task-owned workspace, offline/no-network container, 4 CPU/6 GiB/
  512 PID and 90–300s group bounds, sequential, registry read-only. No production
  or codec execution. Exact commands and environment failures in subissue.
- Focused strict lib/media_formats Clippy, fmt and diff checks pass. Broad strict
  Clippy blocked by existing unrelated lints; new fixture lints fixed; broad run
  allowing documented existing lint classes passes. Full/release/fixture lanes
  not run. Containers removed, isolated source/build retained for review.

## 2026-09-18 — close reviewed local-upload milestones (docs only)

- Parent explicitly approves closure of local rich uploads and its persistence,
  worker, HTTP, browser and media-authorization subissues; moved exactly those six
  links to the archive, preserving detail files and historical evidence.
- Reviews: persistence `688362` approved; worker `2ea39` singleton high resolved;
  HTTP `242746` approved after actual restricted runtime/writer evidence;
  authorization `22156` approved M1/M2. Implementations are committed through
  `6f44264`. This completion overrides earlier local-upload open/uncommitted/
  review-pending handoff notes below, including the browser closure proposal.
- Combined acceptance: all 24 external MIME types in HTTP; recorded worker faults,
  committed replay retention and successful retirement; exact-`6f44264` native
  HEIC/AVIF/video/audio before-attach and followers-only post/reload in uploading
  and fresh owner contexts, anonymous denial, malformed retained 422/raw cleanup.
- No new execution or actual transport-loss/power-loss proof, full fixture/release
  matrix, or remote implementation claim. Frontend rich-media parent stays OPEN:
  remote caching/previews and its remote acceptance criteria remain unmet.
- Preserved pending browser documentation. TDD is inapplicable to docs-only
  reconciliation; checked diff and exact index movement. No code, production or push.

## 2026-09-18 — historical browser handoff: reviewed 6f44264 rerun passed

- Exact clean tracked archive `6f44264d20803f90cc66f11dfc696fa6b44ab736`, default
  binary/no test-support, verified clean Mastodon `1440d55b139e39ec722c2a3db7f60b66cd889048`
  reference and checksum-verified frontend. Fresh task-owned PostgreSQL 14.23 with
  operational migrations 1–5; unchanged narrow runtime/writer grants. No source,
  schema, privilege, production, remote-media/cache/search edits or commits.
- Actual agent-browser 0.31.1 / Chrome 148.0.7778.96: **HEIC, AVIF, WebM and Ogg**
  file input → 202 → worker-held 206 → real-worker 200 → Post 200. All four posts
  followers-only (DB visibility=2); attachment identity preserved.
- Both previous blockers now pass **before attachment**: HEIC/AVIF composer
  thumbnails decode 588×392 and native editor originals 600×400; video poster
  640×360 and editor playback 960×540, clock 0→0.087241s; audio editor Play and
  clock 0→0.041135s. Native playback has readyState=4, no error; tiny video has
  decoded-frame evidence. Muted measurements, not an assertion of audible output.
- Uploading-session post/immediate reload: both images decode; video clocks
  0→0.087094/0.087121s, audio 0→0.040151/0.041139s. Fresh owner browser context
  post/reload also passes all four exact URLs: video 0→0.087181/0.087095s, audio
  0→0.040537/0.040187s. Real owner session state imported only into that fresh
  context, then deleted; no forged credentials, URL changes or cache disabling.
- Browser-origin full/range checks: owner cookie originals/previews 200/206,
  exact 16-byte ranges; anonymous unattached and followers-only media 404 with
  private/no-store + media Vary. Fresh anonymous native HEIC cannot decode.
  Other-owner/expired/revoked/bearer-precedence coverage remains the earlier
  reviewed restricted HTTP matrix, not extra browser cases claimed here.
- Malformed HEIC file input again 202→206→422 with visible exact toast and cleared
  attachment. Retained processing=3, null filename/status, zero ownership rows;
  raw root empty. Four successful media remain processing=2/attached.
- NAS evidence `/srv/workspaces/rustodon-upload-browser-6f44264-alice/evidence/`;
  selected local evidence `target/local-upload-browser-6f44264-alice/evidence/`.
  Includes CLI observation scripts, request projections, native clock/decode JSON,
  SQL assertions, screenshots and hashes. Visually inspected composer/editor,
  immediate/fresh reload and error screenshots. Full limits, immutable image IDs,
  commands/scripts and stage distinctions are in the browser subissue/evidence.
- Driver corrections only: CLI `[required, ref=…]` parsing; third-context CDP
  handshake failure avoided by closing fresh-owner context before anonymous;
  anonymous diagnostic fetches moved from restrictive login CSP to the real SPA.
  No application failure or security-policy relaxation. Final fresh/auth phases pass.
  TDD not applicable to observation-only rerun; no repository harness was added.
- All task containers, PG anonymous volume, network, media root, TLS material and
  temporary owner state removed. Browser/forwarder required bounded SIGKILL at
  teardown; worker/web/DB stopped. Existing tools/cache and evidence retained.
- Updated local browser/auth/HTTP/parent issues with **closure proposals**, not
  automatic archives. Parent supplied Review2 approval of committed fix; parent
  should verify combined recorded fault/review criteria before closure. No full
  matrix, peer, remote-media/cache/search or release-completion claim. Docs remain
  uncommitted for parent verification.

## 2026-09-18 — media authorization Review1 compact follow-up (uncommitted)

- Review1 found no blocker/high; addressed only M1/M2. Production follow-up is
  confined to `src/web.rs`; repository/schema/grants and shared auth are unchanged.
  M1 finalizes all recognized media 4xx/5xx privately, including anonymous requests;
  anonymous public 200/206/304 keep the existing public cache policy. The HTTP
  assertion helper now checks cache/Vary for every request, not just credentials.
- M2 selects the existing optional bearer viewer for attached media outside limited
  federation. Malformed/unknown/application-only bearer retains anonymous attached
  access; revoked/expired/scope errors retain their prior errors. Explicit headers
  never enter the cookie path. Unattached grants and limited-federation bearer
  requests still require a functional user. Reads of the small attachment auth
  projection now precede viewer selection, not metadata/file/range authorization.
- TDD actual restricted-role NAS REDs, same focused selector/flags below:
  `review1-m1-red.log`: anonymous unattached denial had no Cache-Control, expected
  private/no-store. After only M1, `review1-m2-red.log`: attached public empty bearer
  + owner cookie yielded 401, expected historical 200. No setup or assertion
  failures were relabelled as product REDs.
- GREEN `review1-final.log`: **1/1**, 25.77s:
  `cargo test --locked --offline --all-features --test media_state local_upload_http::local_upload_browser_media_access -- --ignored --exact --nocapture --test-threads=1`.
  Adds public-attached malformed/empty/unknown/application-only/disabled bearer
  compatibility, revoked/expired/scope errors, private attached no-cookie-fallback,
  anonymous metadata/range denials, public success cache preservation, and a
  sequential limited-federation server proving absent/invalid/app-only/revoked
  credentials deny despite an owner cookie while valid owner bearer/cookie succeed.
- Fresh-restore real-codec regression `review1-lifecycle.log`: **1/1**, 154.25s:
  `cargo test --locked --offline --all-features --test media_state local_upload_http::local_rich_upload_http_lifecycle -- --ignored --exact --nocapture --test-threads=1`.
  Pure web regression `review1-web-unit.log`: **91 passed, 2 ignored**:
  `cargo test --locked --offline --all-features --lib web::tests:: -- --test-threads=1`.
  `review1-clippy.log`: passed
  `cargo clippy --locked --offline --all-features --lib --test media_state -- -D warnings`.
  Local fmt/diff checks passed; final local/NAS source hashes match
  `review1-source.sha256` in `target/media-viewer-evidence/`.
- Reused the prior isolated workspace/run/reset scripts and immutable codec/PG14
  images, syncing only the two explicitly edited source/test files. Same tools
  bounds and unchanged runtime/writer grants; new task PG lifetime 3600s with the
  same 1 CPU/512 MiB/128 PIDs/no host ports. Gates ran sequentially; only this
  invocation's PG container/anonymous volume and network were removed. No task
  containers remain. No browser, production, deployment or commits.
- Paused for parent Review2; no further implementation expansion or edits pending
  that review. Issue remains open and all changes uncommitted.


## 2026-09-18 — scoped local-upload browser media reads (uncommitted)

- Base `373da56`; created/indexed `fix-local-upload-browser-media-authorization`
  before code, under the existing browser/upload issues. Production changes are
  only web + repository. No schema/grants/jobs/global auth/CORS/signature/CSRF,
  production, deployment, or browser execution.
- Paperclip GET/HEAD authenticates explicit bearers with READ_STATUSES + require_user;
  only absent Authorization permits session lookup and internal backing-token
  authentication. Session user/account identities and functional state must agree.
  No session touch, token mint/exposure, or cookies. Empty Authorization also gets
  private denial caching and never falls back.
- Small authorization projection retains media.status_id separately from joined
  status availability. Only ready (`processing=2`), local, truly unattached owner
  media is newly readable. Attached historical processing/status visibility and
  report-manager discarded exception remain. Exact path metadata and openat2 are
  unchanged. Authenticated recognized-route responses, including all denials, get
  private/no-store + Vary Authorization, Cookie, Signature.
- TDD NAS RED `red.log`: pre-fix production, ready unattached owner bearer GET
  **404 vs expected 200**. Additional baseline replay `red-cookie.log`: final test
  against exact base web/repository, private attached owner cookie GET **404 vs
  expected 200**. Both used persisted tokens/sessions and restricted roles.
- Final GREEN `final.log`: **1/1**, command
  `cargo test --locked --offline --all-features --test media_state local_upload_http::local_upload_browser_media_access -- --ignored --exact --nocapture --test-threads=1`.
  Covers original/small GET/HEAD/range/conditional, owner bearer/cookie, other owner,
  pending/failed stale files, missing/remote/raw, private owner/follower/unrelated,
  deleted/dangling + report manager, expired/revoked/logout/disabled/2FA/scope/mismatched
  sessions, malformed/empty/invalid/scoped/application-only bearer precedence,
  metadata/disk denials, no Set-Cookie and full session/token snapshot invariance.
- `lifecycle.log`: **1/1**, same command flags selecting
  `local_upload_http::local_rich_upload_http_lifecycle` (168.63s), fresh restore.
  Existing real-codec lifecycle extended to owner bearer/cookie GET/HEAD/range for
  every ready original/preview URL across the 24 advertised external formats.
  Not a second codec harness. Subsequent test-only edits added the independent
  attached-cookie assertion and simplified a path-denial assertion; lifecycle
  code and production source were unchanged after its passing run.
- `final-web-unit.log`: `cargo test --locked --offline --all-features --lib web::tests:: -- --test-threads=1`
  **91 passed, 2 ignored**, including existing pure media/cache/range tests.
  `final-clippy.log`: `cargo clippy --locked --offline --all-features --lib --test media_state -- -D warnings`
  passed. Local fmt/diff checks passed.
- Preserve rather than silently change a discovered compatibility convention:
  Rack's unsatisfiable 416 cascades to this server's existing **404**. Tests now
  require that denial to be private; no claim that an HTTP 416 was emitted.
  Initial `green.log` exposed the test's incorrect 416 expectation. `green2.log`
  exposed fixture ON DELETE SET NULL (not a dangling reference); setup now uses an
  owner-only transaction to install a genuinely dangling ID in the disposable DB.
  `green3.log` exposed an invalid/unrecognized style, removed from the recognized-route
  cache assertion. No schema/grant or range parser change was made to fix tests.
- Workspace `/srv/workspaces/rustodon-media-viewer-373da56-alice/`; selected logs,
  run/reset scripts, grant provenance, baseline source and final hashes also under
  local ignored `target/media-viewer-evidence/`. Source sync used tracked files only,
  followed by exact edited files; no environments, credentials, .git or build output.
  Runtime image `7203e0222e2bb72e0b83ab623051873604874cc41a688e318b84691bfa77ad8a`
  (Rust 1.97.1 / FFmpeg 7.1.5); PG14 image
  `1a6c2409ab71f4d054d676ba09d9b74b5d843d805bd5a0cf08314d27ca659d37`.
  Tools sequential: 4 CPU/6 GiB/512 PIDs/870s +900s outer (+15s kill), read-only
  source/root, no capabilities, bounded tmpfs, existing authorized cargo caches.
  Task-only PG/network `media-viewer-pg-alice` / `media-viewer-alice`: 1 CPU/512 MiB/
  128 PIDs/7200s, no host ports, fresh restored database per selector. Existing
  runtime/writer grant contract and actual SQLSTATE 42501 rejection assertions.
- Verified local web/repository/test SHA-256 against the final NAS snapshot.
  Removed only task PostgreSQL container/anonymous volume and network; task media
  roots were in per-run tmpfs. No task containers remain; logs/source retained.
- Independent review remains for parent (nested task delegation unavailable at
  this depth). Keep subissue and parents open and all changes uncommitted. Browser
  rerun and full matrix/release evidence are explicitly deferred.


- Created/indexed [browser subissue](meta/issues/verify-local-rich-upload-browser.md)
  before work. Tested clean tracked `c6cf7b3` archive, default-feature binary with
  no test-support, exact clean Mastodon `1440d55b139e39ec722c2a3db7f60b66cd889048`
  reference and checksum-verified tracked frontend. No application/harness source
  edits, grants/schema changes, production access, remote-media work or commits.
- Real agent-browser 0.31.1 / Chrome 148.0.7778.96: HEIC, AVIF, WebM and Ogg via
  composer file input, 202 → held-worker 206 → real-worker 200, then Post 200.
  Public video and audio have actual advancing native playback clocks before and
  after reload (video also three decoded frames/960×540). Public AVIF decodes in
  fresh browser + reload and eventually in the original uploading browser + reload.
- **Not full acceptance:** ready unattached composer image/video previews and audio
  Edit → Play return 404. Cookie-authenticated native media also returns 404 for
  the author's followers-only HEIC post, while owner bearer range gets 206. Source
  diagnosis: media access requires attached status; optional viewer is bearer-only.
  Public AVIF initially reused a failed image across reload, despite successful
  same-URL fetch; later normal reload decoded. Exact cache mechanism not proven.
  Reported blockers before expansion; no authorization weakening or fix attempted.
- Malformed HEIC via input reaches terminal 422 with visible error toast, clears
  composer attachment, retains processing=3/null filename, and cleans ownership/raw
  files. Browser-origin auth/range diagnostics: public files 206 with correct 16-byte
  range in anonymous/cookie/bearer modes; private HEIC 404/404/206 respectively.
- Task-owned NAS PostgreSQL 14.23, narrow runtime/writer, unchanged grants; bounded
  codec image `7203e022…`. Existing CLI image had no browser: task-only offline image
  `d6337b96…` combines cached immutable CLI/Playwright images. Existing TLS helper
  uses exact SPKI trust plus bounded loopback-only canonical-port forwarding.
  Full identities, limits, setup corrections, results and exclusions are in subissue.
- Evidence: NAS `/srv/workspaces/rustodon-upload-browser-c6cf7b3-alice/evidence/`;
  selected local files `target/local-upload-browser-c6cf7b3-alice/evidence/`.
  Visually inspected video/audio reload, fresh AVIF reload and malformed-error PNGs;
  JSON clock/decode results prove behavior beyond screenshots. Task containers,
  PostgreSQL anonymous volume, network and media root removed; evidence/source/task
  browser image retained. Some containers required bounded SIGKILL during teardown.
- TDD not applicable to this observation-only slice; no reusable repository harness
  added pending source fixes. Independent parent review pending (nested delegation
  unavailable). No full browser/cutover/source-contract/HTTP/peer matrix claim;
  browser subissue and both upload parents remain open. Changes left uncommitted.

## 2026-09-18 — local-upload restricted-role evidence follow-up (uncommitted)

- Evidence/test wiring only while parent independently reviews implementation.
  All `src/**/*.rs` hashes match the start of this follow-up; no feature, schema,
  privilege, production, remote/search, openat2, browser, or commit changes.
- The existing HTTP lifecycle now requires three explicit, distinct URLs. Owner
  (`RUSTODON_OPERATIONAL_DATABASE_URL`) is used only for fixture setup/assertions;
  runtime (`RUSTODON_WORKER_DATABASE_URL`) serves HTTP reads/shared limits, dispatches
  outbox work, and owns the queue; writer (`RUSTODON_WORKER_WRITE_DATABASE_URL`)
  serves HTTP mutations and the production-registered worker handlers. Web wiring
  includes the runtime queue just like production. No owner fallback exists.
- Fresh PostgreSQL roles `upload_runtime` and `upload_writer` use LOGIN/NOINHERIT,
  NOSUPERUSER/NOCREATEDB/NOCREATEROLE/NOREPLICATION/NOBYPASSRLS, no memberships, and
  no database/schema/application-object ownership. Applied unmodified
  `docs/mastodon-refresh-instances.sql` and `docs/mastodon-writer-grants.sql`.
  Runtime SQL is extracted verbatim from the existing standalone
  `src/bootstrap.rs::apply_runtime_grants` profile, substituting only task-owned
  identifiers; extraction script/SQL and grant logs are retained. No extra grants.
- Added a fail-closed role guard plus actual denied-SQL checks. Runtime cannot
  UPDATE public media or SELECT private upload ownership; writer cannot SELECT
  heartbeats, INSERT durable jobs, or UPDATE account private keys. Each returns
  PostgreSQL **42501**. The deliberate owner-as-runtime negative run fails before
  HTTP with `runtime must not inherit or own application objects`
  (`owner-fallback-red.log`). No unexpected denial occurred in application paths;
  no implementation or grant correction is proposed.
- Added owner-only schema setup and a focused real-worker readiness test. The latter
  runs `run_until_shutdown` with actual narrow pools and production handler registry:
  generic Maintenance, writer without root, and local handlers on Pull do not
  advertise local capability; configured local Maintenance does. It checks the
  same predicate used by writable `admin worker-readiness`, periodic emitted
  heartbeat refresh, generic lane/scheduler readiness, and cleanup on shutdown.
  This is not a claim that the separate CLI/startup fixture lane ran.
- Actual final commands (all `--locked --offline --all-features`, serial):
  - `cargo test --test media_state local_upload_http::local_rich_upload_http_lifecycle -- --ignored --exact --nocapture --test-threads=1`
    — **1/1**, `restricted-http-final.log`, all 24 advertised external formats with
    real codecs and the prior full bounded HTTP lifecycle, including success/failure,
    edits, deletion/replay, scopes/owner separation, attachment eligibility, raw
    non-exposure/cleanup, validation, and v1/v2 legacy JPEG success.
  - `cargo test --test media_state local_upload_http::local_rich_upload_restricted_worker_readiness -- --ignored --exact --nocapture --test-threads=1`
    — **1/1**, `restricted-readiness-final.log`, four real handler/lane cases.
  - `cargo clippy --test media_state -- -D warnings` — pass, `roles-clippy.log`.
    Local `cargo fmt --all --check` / `git diff --check` pass.
- Test-wiring corrections are recorded, not hidden application failures. Initially
  owner-side `migrate` rejected the documented writer ACLs because no writer context
  was supplied; changed only setup to `migrate_with_writer_role` using the actual
  writer login (`setup-writer-context-failure.log`). Initial readiness assertions
  raced the separate scheduler heartbeat write; bounded observation now waits for
  both (`readiness-observation-race.log`). Initial PostgreSQL readiness observed its
  temporary init server before database creation; setup subsequently waited for a
  successful TCP SQL connection to the task database before restore.
- Isolated NAS workspace: `/srv/workspaces/rustodon-upload-http-roles-dee7526-alice/`.
  Image `7203e0222e2bb72e0b83ab623051873604874cc41a688e318b84691bfa77ad8a`
  (Rust 1.97.1 / FFmpeg 7.1.5); PostgreSQL 14.23 image
  `1a6c2409ab71f4d054d676ba09d9b74b5d843d805bd5a0cf08314d27ca659d37`.
  Sequential tools runs use 4 CPUs, 6 GiB memory/swap, 512 PIDs, read-only root/source,
  dropped capabilities/no-new-privileges, bounded tmpfs, 870s container and 900s outer
  wall bounds (+15s kill). PostgreSQL uses only `upload-http-roles-pg-alice`, network
  `upload-http-roles-alice`, database `uploads`, with 1 CPU, 512 MiB memory/swap,
  128 PIDs, 3600s lifetime and no published ports. Fresh restore/grants before each
  final gate; existing authorized cargo/target caches reused. `run.sh`,
  `reset-fixture.sh`, grant provenance, logs and hashes are retained in evidence.
- Verified final tested source/grant hashes against the NAS copy and all implementation
  hashes against the start of the follow-up. Removed only this task's PostgreSQL
  container/anonymous volume and network; no task containers remain. Source, grant
  provenance, logs, and caches were retained. Final post-run test-header wording
  changed only documentation; focused strict Clippy also passed on that snapshot.
- Earlier owner-connected evidence is not relabelled: these are additional actual
  restricted-role results. Full matrix/browser/release acceptance remains unclaimed;
  pending independent parent review and separate browser acceptance, issues stay open.

## 2026-09-18 — bounded local rich-upload HTTP slice (uncommitted)

- Started from clean main `dee7526`; created/indexed
  `integrate-local-rich-upload-http.md` beneath `support-local-rich-media-uploads`
  before source edits. Prior persistence/worker evidence was read and reused.
  No migration, grants, remote/search/browser, production, or openat2 changes.
- Accepted terminal abandonment now retains `processing=3` and filename-null public
  state. Exact-owner cleanup can retire its raw/output manifest without deleting the
  pollable failure row; unlink failure retains ownership for retry. Late publication
  and replay cannot resurrect explicitly deleted or failed media.
- V2 external MIME acceptance uses authenticated transaction-level staging under the
  account lock, private durable raw write/fsync, then atomic acceptance/process intent.
  Staging stays processing=0; accepted processing=1 is immediately owner-visible.
  Ambiguous staging/accept commits reload the exact generation and never infer rollback
  for destructive cleanup. Failed/unknown outcomes retain the manifest.
- POST 202 has a stable ID and null URL/preview. Owner GET/PUT returns 206 pending,
  GET 200 ready, and failed GET/PUT returns exact 422
  `Error processing thumbnail for uploaded media`. Other owners see 404. Failure is
  rechecked under the writer lock; rejected PUT cannot mutate it. Pending metadata
  survives publication. Ready status eligibility still requires processing=2.
- Legacy v1/JPEG success remains 200. Image decoding/resizing/preview normalization
  now runs via `spawn_blocking`; raw acceptance hashing is also off the web executor.
  Raw filesystem write/fsync remains synchronous under the account lock so cancellation
  cannot leave a detached writer racing durable cleanup. Processor child limits remain.
- Modern stills intentionally use the asynchronous composer workflow rather than
  reproducing pinned synchronous modern-still processing. Cheap declared MIME/size
  failures return 422 without a row/owner/file; byte/MIME mismatch can instead return
  202 then terminal 422. Null previews are not invented before real artifacts exist,
  and audio without a generated preview remains null.
- Worker heartbeats advertise local-upload capability only when both local handlers
  register on a selected Maintenance lane. Writable `admin worker-readiness` now
  requires this fresh capability in addition to normal lane/scheduler readiness.
  `/ready` remains the existing database-only check, not a codec or browser gate.

### TDD and executed evidence

- `red.log`: `terminal_upload_retains_failed_row_without_filenames` failed against
  the baseline with missing public row (`None` instead of processing=3).
- `api-red.log`: the new HTTP lifecycle exercised the unchanged NAS baseline and
  failed on HEIC POST (422 instead of 202). HTTP fixture source was developed
  alongside integration; not every branch had a separate preimplementation red run.
- Final bounded runs (all commands use `--locked --offline --all-features`):
  - `cargo test --test media_state local_rich_upload_http_lifecycle -- --ignored --nocapture --test-threads=1`
    — **1/1**, `api-final.log`, including all **24 advertised external MIME types**
    through actual codecs. Held-worker pending polling, raw HTTP non-exposure,
    initial metadata/pending edits, cross-owner/scopes, pending attach refusal,
    ready attach success, actual artifact paths, optional audio preview, stable ID,
    failure retention/no-mutation, pending/failed deletion, worker replay, no cheap
    validation orphans, and legacy v1/v2 JPEG success are covered. Final lint-only
    follow-up changed one module documentation word to Markdown backticks.
  - `cargo test --test local_uploads -- --ignored --nocapture --test-threads=1`
    — **5/5**, `repository-final.log`, including narrow restricted-role contracts.
  - `cargo test --lib worker::local_uploads::tests -- --ignored --nocapture --test-threads=1`
    — **9/9**, `worker-final.log`: existing real codecs, commit faults, stale claims,
    cancellation, deletion, retry exhaustion, recovery bounds, plus retained terminal
    cleanup failure/replay and local-capability readiness/staleness.
  - `cargo test --test media_state pinned_media_state_http_matrix -- --ignored --exact --nocapture --test-threads=1`
    — **1/1**, `legacy-http-final.log` (existing fourteen-request contract matrix).
  - `cargo test --test workers local_media_jobs_reconcile_create_and_delete_crash_boundaries -- --ignored --exact --nocapture --test-threads=1`
    — **1/1**, `legacy-worker-final.log`.
  - `cargo test --lib web::tests -- --test-threads=1` — **91 passed, 2 ignored**,
    `web-unit.log`; `cargo test --test media_formats --test media_processor -- --test-threads=1`
    — **3 + 7 passed, 3 ignored**, `media-unit-final.log`.
  - `cargo clippy --lib --bin rustodon --test local_uploads --test media_state -- -D warnings`
    — pass, `clippy-final.log`. Local formatting and diff checks pass.
- Setup corrections, not hidden green results: repeated initial HTTP runs exhausted
  the real shared 30-upload rate limit; final runs use a fresh task-owned restore,
  with no limiter bypass. The readiness staleness fixture needed both timestamps
  aged to obey the existing heartbeat constraint. One legacy worker invocation
  omitted its ADMIN database variable and was rerun correctly. The tools image has
  no external `kill` executable: initial supervisor cancellation test errored on
  executable lookup. Its passing rerun used a task-container `/tmp/test-bin/kill`
  script delegating to `/bin/sh`'s real `kill "$@"` builtin; no fake result or
  processor change, system installation, or image mutation.

### Environment and limits

- Workspace/evidence: `/srv/workspaces/rustodon-upload-http-dee7526-alice/`.
  Supplied immutable media-tools image verified as
  `7203e0222e2bb72e0b83ab623051873604874cc41a688e318b84691bfa77ad8a`;
  PostgreSQL 14.23 image
  `1a6c2409ab71f4d054d676ba09d9b74b5d843d805bd5a0cf08314d27ca659d37`.
- `evidence/run.sh` reuses the prior bounded native tools pattern: sequential runs,
  4 CPUs, 6 GiB memory/swap, 512 PIDs, 870s Podman / 900s outer wall bound (+15s kill),
  read-only source/root, dropped capabilities, no-new-privileges, bounded tmpfs.
  PostgreSQL used only `upload-http-pg-alice`, `upload-http-alice`, and database
  `uploads`, with 1 CPU, 512 MiB memory/swap, 128 PIDs, 7200s container timeout,
  bounded readiness/restore, and no published ports. Reused authorized cargo/target
  caches at `rustodon-null-route-20260918`; did not alter unrelated resources.
- Only tracked source and explicit new fixture files were synchronized; no instance
  environments, credentials, `.git`, build output, or reference checkout. Final
  source hashes/verification and resource cleanup logs accompany the evidence.
- HTTP and worker integration used the disposable fixture **owner** connection;
  legacy URL variables were explicitly pointed to that same task-owned database.
  The separate restricted-role persistence test is not end-to-end least-privilege
  HTTP/worker proof. No full named media/worker/schema/startup/peer/browser gate,
  release claim, actual transport-loss/power-loss acceptance simulation, or new
  pinned-source run. Parent-supplied exact-source verification remains separate
  evidence; HTTP commit-reload code is not claimed as live transport-fault proof.
- Verified runtime versions: Rust 1.97.1, FFmpeg/ffprobe 7.1.5, PostgreSQL 14.23.
  Removed only this task's PostgreSQL container/anonymous volume and network; no
  task containers remain. Retained source/evidence and the shared authorized caches.
- Left uncommitted for parent independent review. HTTP subissue and parent remain
  open pending review and separate composer preview/playback/post/reload acceptance.

## 2026-09-18 — local upload review round 1

- Addressed reviewer `ea39b9de`'s high scheduler finding only. The maintenance tick
  now calls shared `schedule_recovery`, whose root singleton has the stable
  `local-upload-recovery:root` key; cursor continuation keys remain unchanged.
- TDD: `recovery_scheduler_second_tick_reuses_live_root` reproduced
  `InvalidData("singleton jobs require a logical key")` before the key fix. It
  invokes the actual scheduling path twice while root is live, checks `Existing`
  ownership of the same job, and checks exactly one live root.
- Reused the bounded NAS image/runner and a newly created task-owned restored
  PostgreSQL fixture (this run also sets a 3600s database-container timeout).
  `--lib worker::local_uploads::tests` passed 7/7; existing worker regression
  `local_media_jobs_reconcile_create_and_delete_crash_boundaries` passed 1/1;
  focused strict Clippy passed. Exact flags match the prior entry. Evidence:
  `review1-red.log`, `review1-green.log`, `review1-legacy.log`, `review1-clippy.log`
  in `/srv/workspaces/rustodon-upload-worker-949d503-alice/evidence/`.
- Local `cargo fmt --all --check` and `git diff --check` pass. Terminal failure
  retention and HTTP v2 remain untouched. Left uncommitted for parent review.

## 2026-09-18 — local upload worker and recovery slice (uncommitted)

- Created/indexed `process-and-recover-durable-local-uploads.md` before code,
  beneath the rich-upload parent and reviewed persistence boundary at `949d503`.
  No HTTP v2 wiring, migration, public DDL, grants, new queue lane, production
  operation, or macOS weakening. Independent parent review remains outstanding.
- Added Maintenance/Media processing and bounded recovery, a private mode-0700
  raw namespace using existing confined filesystem primitives, exact length/hash
  reads, bounded processor integration outside account locks, manifest-before-I/O
  installation, stale/account fences, committed replay, and raw-only ready-owner
  retirement. Recovery retains live/undispatched work, scans 100 identities per
  keyset page, and continues past failed owners. Legacy rollback cleanup now
  spares durable staging. Existing table state sufficed for this slice.
- TDD: the ready-retirement test failed with missing `retire_ready_in`, then
  passed. Deterministic worker fault tests were added alongside the integration;
  not every worker branch had a separate preimplementation red run. Reusing the
  supplied target cache initially selected an older library (missing the already
  committed `mastodon::local_uploads` export); touching the isolated source's
  `src/lib.rs` forced recompilation before the intended red and final gates.
- NAS workspace/evidence:
  `/srv/workspaces/rustodon-upload-worker-949d503-alice/{source,evidence}`.
  Immutable tools image verified as
  `7203e0222e2bb72e0b83ab623051873604874cc41a688e318b84691bfa77ad8a`.
  PostgreSQL 14.23 image `1a6c2409ab71f4d054d676ba09d9b74b5d843d805bd5a0cf08314d27ca659d37`;
  only task `upload-worker-pg-alice`, network `upload-worker-alice`, and database
  `uploads` were used. Tracked source plus explicit new files only were copied;
  no instance environments, credentials, build output, `.git`, or reference tree.
  Reused authorized `rustodon-null-route-20260918` cargo/target caches.
- `evidence/run.sh` records sequential tools runs with 4 CPUs, 6 GiB memory/swap,
  512 PIDs, read-only root/source, dropped capabilities, no-new-privileges, bounded
  tmpfs, Podman timeout 870s and outer timeout 900s (+15s kill). PostgreSQL used
  1 CPU, 512 MiB memory/swap and 128 PIDs; readiness/restore were bounded at
  30s/120s. No host ports or production resources were used.
- Exact successful final commands inside that runner:
  - `cargo test --locked --offline --all-features --lib worker::local_uploads::tests -- --ignored --nocapture --test-threads=1`
    — 6/6, `worker-final.log`. Real PNG/WebM/Ogg succeeded through the registered
    `WorkerExecutor`, with public artifacts retained after raw retirement/replay.
    Fault coverage: partial writes, before/after publication commit faults,
    raw unlink failure, size/hash mismatch, symlinks, stale generation/claims,
    deletion/account disable/edit during unlocked processing, cancellation,
    real queue retry exhaustion, legacy staging guard, and paginated recovery
    progressing despite a failed unlink.
  - `cargo test --locked --offline --all-features --test local_uploads -- --ignored --nocapture --test-threads=1`
    — 4/4, `repository-final.log`, including ready retirement, migration/restricted
    role contracts and the previous ownership regressions.
  - `cargo test --locked --offline --all-features --test workers local_media_jobs_reconcile_create_and_delete_crash_boundaries -- --ignored --exact --nocapture --test-threads=1`
    — 1/1, `legacy-worker-final.log`. Its three worker database URL variables were
    explicitly set to this disposable owner fixture, not production or claimed
    restricted runtime identities.
  - `cargo clippy --locked --offline --all-features --lib --test local_uploads -- -D warnings`
    — pass, `clippy-final.log`. Local `cargo fmt --all --check` and
    `git diff --check` pass. This is focused lint, not the all-targets gate.
- Verified SHA-256 equality for all six changed/new Rust files against the tested
  NAS source (`code-sha256.txt`, `source-verification.log`). Removed only the task
  PostgreSQL container/anonymous volume and task network; retained source/evidence
  and shared caches. No task test containers remain.
- Evidence boundary: no full worker/media/browser/peer matrix, HTTP v2, release
  claim, real transport-loss/power-loss simulation, or end-to-end least-privilege
  worker gate. Existing fail-before/fail-after commit hooks exercise durable
  replay states. The processor foundation gate supplied by the user is separate
  evidence, not rerun or substituted for these worker cases.
- Follow-up API contract is recorded in the new subissue. In particular, current
  terminal abandonment deletes pending media, not a retained failure reason;
  agree on compatible polling/error semantics before enabling v2. Stage-before-
  raw-write and accepted-intent-before-response, authenticated pending edits and
  delete, raw non-exposure, and HTTP/composer attach/play/reload remain next work.

# Development Log

## 2026-09-18

- Added the first bounded durable-local-upload ownership slice (pending parent
  review, no live upload/worker integration). Migration 5 owns exact raw/input and
  output cleanup identities independently of public media deletion; transaction
  APIs fence staging/acceptance/claims/manifests/publication and preserve edited
  metadata. No public schema, legacy synchronous-path, queue-lane, production, or
  Linux filesystem changes. Bootstrap uses existing migration/grant integration.
- TDD evidence: the initial focused test first encountered a missing-public-schema
  setup error on an empty DB (not the intended red). After restoring the tracked
  pinned `fixtures/mastodon/v4.6.5/database.sql`, it failed specifically with
  `filename-null rollback staging has no durable upload owner`. The expanded
  lifecycle tests then passed after implementation/fixture fixes; those expanded
  cases were not individually run before their corresponding primitives existed.
- Linux verification used task-owned `upload-ownership-pg` / network
  `upload-ownership-test`, PostgreSQL 14.23 image `1a6c2409ab71`, tools image
  `localhost/rustodon-browser-tools:remaining` (`dd8b417b66aa`), and task source
  `/srv/workspaces/rustodon-upload-ownership/source`. Only tracked source and
  explicit new files were copied; no instance configuration or reference checkout
  was copied. Reused the supplied cargo-home/target caches. Test containers ran
  sequentially under `timeout 900`, `--cpus 4 --memory 6g --pids-limit 512`;
  PostgreSQL had `--cpus 1 --memory 512m --pids-limit 128`. No host ports exposed.
  Original local Mastodon reference was clean at exact
  `1440d55b139e39ec722c2a3db7f60b66cd889048`.
- Exact successful test commands inside the bounded Linux tools container:
  - `cargo test --locked --test local_uploads -- --ignored --nocapture --test-threads=1`
    (3/3; restored disposable `uploads` database, admin fixture connection).
  - `cargo test --locked --test operational_schema migration_plan_requires_an_exact_known_prefix -- --exact`
  - `cargo test --locked --test operational_schema operational_schema_lifecycle_is_isolated_and_idempotent -- --ignored --exact --nocapture`
    (fresh migration, rerun, concurrency, catalog drift/rejection; 1/1).
  - `cargo test --locked --features test-support --test standalone_bootstrap standalone_bootstrap_installs_and_verifies_exact_baseline -- --ignored --exact --nocapture --test-threads=1`
    (1/1; separate empty `upload_bootstrap` database, dedicated non-superuser
    installer/runtime/writer roles, task-only media root). This includes exact
    bootstrap verification, restricted writer/runtime checks, rejection cases,
    and the existing synchronous HTTP smoke. No `PF_MEDIA_PROCESSOR` tail failure
    occurred; this is not a real-codec capability gate or the full launcher lane.
  - `cargo clippy --locked --all-features --lib --test local_uploads --test operational_schema -- -D warnings`
  - Local `cargo fmt --all --check` and `git diff --check`.
- Final focused rerun passed (3/3) and focused strict Clippy passed with verified
  `rustc 1.97.1 (8bab26f4f 2026-07-14)`. Removed only the task PostgreSQL
  container/its anonymous volume, task network, and task media directory; retained
  source workspace and shared caches. Changes remain uncommitted for parent review.
- Broader `cargo clippy --locked --all-targets --all-features -- -D warnings`
  encountered existing unrelated `paperclip.rs:2901` needless-by-value lints and
  `tests/mastodon_schema.rs:8971` test-length lint, plus a new lifecycle-test-length
  lint. Only the new fixture was adjusted, using the existing tests' scoped
  `too_many_lines` allowance; focused strict Clippy then passed. Unrelated source
  was not changed. Full ordinary/media/worker/browser/peer gates were not run.


- Made public timeline routing null-safe for replies whose parent is not yet
  stored. Previously a nullable SQL predicate failed Rust boolean decoding,
  rolling back inbound posts and blocking later activities from the same actor.
  A bounded Linux/PostgreSQL regression reproduced the failure before the fix
  and verified reply persistence, queued thread recovery, and successor progress
  afterward.

## 2026-09-16

- Added the bounded rich-media processor foundation for every format advertised
  by the frontend. Untrusted bytes now reach forced FFmpeg/ffprobe demuxers only
  through anonymous pipes; child I/O, decoded dimensions, duration, frame count,
  frame rate, threads, aggregate deadlines, and generated output sizes are
  bounded, with cancellation killing and reaping child work. AVIF/HEIC normalize
  to JPEG, video to validated H.264/AAC MP4 plus a real PNG poster, and audio to
  validated MP3. Startup/preflight exercises pinned tiny fixtures and fails
  closed if the required runtime codecs are missing. Local durable upload and
  remote-cache integration remain tracked separately and intentionally open.

- Added standalone instance bootstrap from an empty PostgreSQL 14 database,
  without a Mastodon/Rails runtime dependency. A deterministic schema-only
  Mastodon 4.6.5 artifact and exact migration inventory feed a transactional,
  fail-closed `rustodon admin bootstrap-instance` command that creates fresh
  instance/Owner signing identities, baseline settings, Rustodon operational
  state, and exact runtime/writer grants. Exact reruns verify without rotating
  credentials; partial, drifted, live, or non-empty-media targets are rejected.
  The installer now supports a non-superuser that directly owns the database and
  `public` schema.
- Added the bounded `standalone-bootstrap-integration` PostgreSQL-only lane and
  offline lifecycle harness. The live smoke verifies first-Owner browser login,
  media upload, public status creation, WebFinger, ActivityPub, exact baseline
  and rerun behavior, least-privilege runtime/writer connections, and rejection
  paths. Added a standalone operator runbook covering role/secret provisioning,
  startup/readiness, backups, and the separate existing-Mastodon cutover path.
  The live PostgreSQL 14.23 test passed against a task-owned NAS container; the
  Linux-local runner's selector/environment/timeout/ownership cleanup paths pass
  offline harness coverage. Independent blocker/high review passed.

- Repaired the operational schema v3-to-v4 deployment blocker by registering the
  221-entry PostgreSQL 14.23 catalog hash produced by the exact physical-clone
  rehearsal. The superseded hash came from a newer PostgreSQL catalog whose
  statistics/null, NOT NULL constraint, and `MAINTAIN` representations are
  version-sensitive. The catalog query, ACL/security detail, migration DDL, and
  PostgreSQL 14 support boundary remain unchanged; focused provenance coverage
  pins the exact v4 hash and keeps column statistics in drift detection.

- Added bundled-frontend quote-post creation and federation lifecycle support:
  POST-only `quoted_status_id` parsing and idempotency binding, transactional target
  policy/visibility/block/direct validation, canonical reblog targets, local accepted
  and remote pending state, silent access, counters, streams, notifications, and
  durable QuoteRequest/Accept/Reject/QuoteAuthorization effects. Incoming Notes now
  reconcile quote aliases, authorization, updates, removal, deletion, and revocation
  with actor/host/object/instrument bindings and accepted-boundary accounting.
  Non-author quotes are restricted to distributable public/unlisted targets, and
  QuoteRequest delivery rechecks the locked live pending relationship so deletion
  cancels both outbox and unleased durable work without a stale-delivery race.
  Centralized quote-policy semantics are shared by writes and REST projection.
  Added exact Mastodon 4.6.5 source contracts, least-privilege quote grants,
  restored-schema/worker selectors, a Rails-versus-Rust quote lifecycle differential,
  and an Alice Chromium quote create/reload flow with exact rollback inventory.
  Signed QuoteAuthorization Deletes retain the verified payload and use the quoting
  status' forwarding reach before transactional revocation; legacy quote state
  updates leave counters unchanged. Differential/worker selectors now assert exact
  semantic quote notification, QuoteRequest, author-stream/update, full rollback,
  transition-specific Accept/Reject/revoke effects, remote quoting-Note deletion,
  conflicting replay, and signed forwarding intents; the offline rollback double
  executes mention-sequence restoration. Focused offline selector/browser/rollback
  contracts, pinned-source contracts, default-feature Rust tests, all-feature check,
  and strict Clippy pass.
  Native Rust commands use a temporary macOS Paperclip compatibility patch that is
  restored byte-for-byte; restored database, worker, differential, browser, cutover,
  and peer lanes remain intentionally unrun. The issue stays open pending the
  requested final heavy sweep.

- Hardened quote parity after independent review: viewer-side domain blocks no longer
  reject otherwise writable quote targets (explicit account blocks and author-side
  restrictions remain), while REST disclosure filtering is unchanged. Added the
  frontend `PUT /api/v1/statuses/:id/interaction_policy` contract with pinned
  public/followers/nobody validation, ownership/scope checks, no-op suppression,
  status streams, and independently fenced ActivityPub updates, plus behavioral
  differential coverage for the frontend revoke route. Remote quoted Tombstones now
  include Mastodon's ID-less removal form without dereferencing deleted targets.
  QuoteAuthorization Deletes persist durable URI/actor tombstones before attachment,
  and stale authorization cannot resurrect rejected or revoked relationships.
  The focused worker selector now uses the restricted worker-writer role and covers
  allowed/denied embedded QuoteRequests, Note-before-request rejection, ID-less
  Tombstone removal, stale authorization replay, and signed scalar-instrument fetch
  with same-job retry. Scalar imports are reauthorized under the target-status lock
  before any Note effects. Quote request/decision deliveries carry exact lifecycle
  identity into account → status → quote locks held through bounded HTTP, recheck
  domain policy and the exact durable-job lease generation, and cancel only safe
  unleased/expired terminal work. Malformed delivery metadata dead-letters instead
  of bypassing the fence; exhausted transient scalar fetches converge to Reject. Heavy
  restored-schema, worker, differential, browser, cutover, and peer execution remains
  deferred; no such final-tree pass is claimed. Follow-up review fixed the decision
  fence's Reject-state discriminator and granted the writer only column-scoped
  durable-job lease/timestamp `UPDATE` privileges. The final fence now serializes
  against relationship changes and suppresses delivery after either account blocks
  the other. The focused worker selector now drives signed QuoteRequest/Reject/Accept
  HTTP deliveries and reclaims actively leased request/decision jobs after block or
  terminal transitions to prove wire suppression.
  Differential source now covers blank/omitted policy defaults, invalid private/reblog
  validation, and denied revoke nonmutation; those heavy selectors remain unrun.

- Added the complete bounded poll lifecycle used by the bundled Mastodon client:
  transactional poll creation and status idempotency, visibility-authorized show and
  vote routes, exact option/expiry/grapheme validation, serialized duplicate-safe
  voting, current viewer/tally projections, and signed SSRF-safe refresh of stale
  remote polls. Poll writes retain deterministic account/status/poll lock ordering,
  apply symmetric block and parent-status visibility policy to REST and federated
  votes, and extend least-privilege writer/preflight contracts.
- Added ActivityPub `Question`, vote, and independently versioned poll-update support,
  including inbound identity/idempotency checks, delayed visible-tally fan-out,
  hidden-tally deferral, exact-generation expiration notifications, and bounded
  startup/periodic repair. An immutable database-clock activation boundary baselines
  historical expirations without replaying old notifications or federation; missing
  post-activation work remains repairable across outages. Reconciliation uses fenced
  singleton leases, terminal-marker filtering, bounded keyset continuations, and a
  retry-budgeted raw fallback that cannot create zero-progress job chains. Added
  pinned-source, unit, restored-fixture source, differential, worker, and two-account
  browser workflow coverage. The restored database/worker, differential, and browser
  lanes remain deferred to the final combined Podman sweep.

- Added all nine timeline WebSocket subscriptions used by the bundled Mastodon
  frontend, including parameterized hashtag/list envelopes, exact raw hashtag wire
  casing, compatible subscription errors, REST-selector-backed public/tag/list
  membership filtering, and create/edit/delete projection from audience-independent
  transactional events. Each connection captures its committed cursor before any
  subscriptions; the first authorized subscription retains that lower boundary to
  close the initial socket/subscribe race, while later multiplexed subscriptions use
  a fresh per-command cursor so they do not prepend creates already represented by
  their REST pages. Each authorized timeline subscription captures a second upper
  cursor after authorization and replays through it, so creates racing the actual
  subscribe boundary are not lost. Replay retains bounded historical suffixes of up
  to 128 route-relevant deletes and 128 non-creating membership/edit transitions,
  while returning every event in the finite subscribe handoff window. Historical
  wire-level creates are omitted, including `status.update` route entries;
  idempotent edits and actual deletes still replay across reconnects to close
  disconnect gaps. Structural route filtering happens
  before these per-class limits, so unrelated public, hashtag, or list traffic cannot
  displace a recoverable event. The subscription baseline then skips those same rows
  during live polling. The bundled frontend's ID-based timeline/status reducers make
  repeated creates and updates visibly idempotent; this replay is required for hashtag
  timelines, which have no REST `fillGaps` hook. Lifecycle events retain authoritative
  structural public, hashtag, locality, media, language, canonical tag, and exact list
  routing facts,
  including hard deletion, suspension, domain moderation, and purge transitions.
  Before-snapshot membership stays authoritative across moderation changes, while
  suspension and remote hard deletion fan out user-stream deletes to followers,
  followed-tag viewers, and non-silent mention recipients before availability or
  relationship rows disappear. Stream polling is lock-free while writers remain
  serialized. Large lifecycle and purge transactions stage immutable stream events
  as transaction-private, non-dispatchable outbox rows in bounded Rust batches while
  collecting pre-delete routing facts, then take the global writer-order lock only
  for their deterministic terminal `INSERT ... SELECT` immediately before commit;
  staging rows are deleted in that same transaction. Ordinary status creates, edits,
  boosts, and remote-media reconciliations likewise collect or stage their global and
  recipient fan-out before taking the ordering lock at the terminal flush. Retention
  pruning does not take that lock, and dedicated partial indexes keep stream history
  separate from normal pending outbox dispatch.
- Added parser/envelope/scope and all-nine delete-routing unit coverage, pinned
  frontend/server protocol and reducer-idempotence contracts, restored-WebSocket
  fresh-replay/race/reconnect coverage, and isolated operational replay-bound,
  retention/contention, commit-order, and index regressions. Restored PostgreSQL,
  worker, WebSocket, and browser fixture lanes are deferred to the final combined
  Podman sweep.

- Added insert-only self-healing for missing `account_stats` rows using the existing
  reconciliation rules. Writer-backed web startup repairs one bounded account before
  serving reads and attempts at most 24 additional one-account background batches;
  later mutations continue healing any remainder. Local reciprocal and inbound
  relationship writes pre-heal involved accounts before relationship/lifecycle
  locks. Mutation boundaries otherwise initialize from post-mutation state and apply
  one aggregate delta only when another transaction already won initialization;
  populated counter rows remain authoritative. Missing-stats ActivityPub outbox
  totals use the same direct-status exclusion as repaired rows.
- Added focused restored-schema regressions for repaired account/instance and outbox
  projections, idempotence, preservation of existing rows, deterministic reciprocal
  local writes, inbound relationship writes, cascaded reblog deletion, and subsequent
  counter correctness.

## 2026-09-10

- Implemented the ten concrete release-readiness fixes. ActivityPub actor
  avatar/header URLs now honor relative and absolute `PAPERCLIP_ROOT_URL` values;
  browser CSRF tokens are HMAC-authenticated with `SECRET_KEY_BASE`, use an HTTPS
  `__Host-` cookie, and reject duplicates; public OAuth clients now require S256
  PKCE.
- Added authenticated published announcements and Mastodon-compatible hashtag
  search for the pinned frontend, including optional `read:search` authentication,
  pagination rules, tag relationships, and the v2 response shape.
- Made local media creation and deletion crash-safe with unpublished staging,
  fsynced Paperclip writes/removals, atomic metadata deletion plus durable cleanup
  intents, and retryable reconciliation. Restored worker integration covers both
  database/filesystem publication boundaries.
- Added durable URI-only ActivityPub `Create Note` resolution with signed bounded
  fetches, recipient-scoped deduplication, Mastodon-compatible signer selection,
  tombstone ordering, recipient repair, and replayable forwarding.
- Added inbound and outbound custom emoji federation, including domain-bound
  metadata validation, durable remote image fetch/replacement cleanup, bounded GIF
  decode work, poll-option association, and local emoji ActivityPub resources.
- Defined mail as bounded at-least-once work. Mail jobs persist an opaque stable
  `Message-ID`; legacy queued jobs are lease-fenced and lazily backfilled before
  SMTP. A deterministic post-acceptance completion fault proves that retries may
  duplicate delivery without silently losing an accepted message.
- Added pinned-frontend source contracts and CI jobs for schema, operational,
  worker, and high-value Rails-versus-Rust differential integration. Actor
  differential coverage exercises avatar and header URLs under relative and
  absolute media roots. `mise run check`, pinned-source contracts, schema 37/37,
  worker 49/49, and browser integration pass. The differential suite contains
  21 unique cases and 22 gate executions; a broad run passed 17 cases before the
  local command timeout, with its remaining `rest_protocol_contracts` and
  `write_transactions` cases passing separately. The isolated notification and
  authorization phases also pass. Live Rails execution of URI-only Create
  remains open.

## 2026-09-09

- Closed an authenticated browser-login open-redirect edge case: `return_to`
  values beginning with `/\\` could be normalized by browsers as an external
  authority. The validator now rejects backslashes, with a regression test;
  `mise run check` passes.
- Added restored-fixture coverage for ambiguous remote-media metadata commits.
  A test-support fault now exercises both a rollback before `COMMIT` and an
  error after PostgreSQL commits; staged Paperclip files remain available for
  retry reconciliation, and the durable jobs finish without leaked leases.
  `mise run worker-integration` passes 45/45.
- Added an eight-wave, twenty-user durable-worker executor soak with four remote HTTP
  permits, retry-after-commit idempotency, exact-key, dead-letter, and queue
  drain assertions. End-to-end production load remains open by design.

## 2026-09-07

- Completed the browser logout surface: every Rust-owned settings page now
  renders a CSRF-protected accessible form, HTML logout redirects to sign-in,
  and JSON logout returns Mastodon's `redirect_to` response while deleting the
  browser session and its OAuth token. The browser authentication and account
  settings differential cases pass against the pinned Mastodon 4.6.5 fixture.
- Aligned inbound remote Note interaction counts with Mastodon: ActivityStreams
  `likes`/`shares` collections and legacy count fields are stored as bounded
  untrusted values in the `0..100_000_000` range. Unit coverage includes
  negative and oversized counts, and restored worker integration remains 43/43.
- Matched Mastodon's case-insensitive HTTP(S) WebFinger resource parsing. The
  guarded federation-discovery differential case now covers a mixed-case URL
  scheme and passes against the pinned 4.6.5 fixture.
- Preserved stored media `blurhash` values in outbound ActivityPub Note
  attachments. A focused serializer regression and the guarded federation
  discovery differential case pass.
- Preserved Mastodon's original media `width` and `height` fields in outbound
  ActivityPub Note attachments, while omitting missing or malformed dimensions.
  Serializer coverage and the guarded federation discovery differential case
  pass.
- Outbound ActivityPub Note attachments now expose materialized thumbnails as
  Mastodon-compatible `Image` icons, including cache-aware Paperclip URLs;
  remote thumbnail URLs without a stored thumbnail file remain omitted.
- Outbound ActivityPub Notes now serialize Mastodon's automatic quote
  `interactionPolicy`, mapping public, followers, and following approval bits
  to their ActivityStreams collections and falling back to the author actor
  when no recognized bit is enabled. Unit and federation differential coverage
  pass.
- Accepted remote quote rows now expose Mastodon's `quoteAuthorization` URI on
  outbound web and durable-worker Notes. Pending, rejected, and missing approvals
  remain omitted. The quoted fixture differential case and worker integration
  pass.
- Added Mastodon-compatible local `QuoteAuthorization` ActivityPub documents at
  both username and numeric account routes. Accepted quote state, target-status
  visibility, and deleted-object checks are enforced before serialization; the
  guarded federation differential case covers both routes.
- Preserved valid media focus metadata as ActivityPub `focalPoint` lists while
  omitting incomplete or malformed focus values. Serializer coverage covers
  both branches.
- Matched Mastodon's account-level sensitivity behavior for outbound Notes:
  sensitized authors now mark their Notes sensitive even when the status itself
  is not flagged.
- Preserved Mastodon's `_misskey_quote` alias alongside `quote` and `quoteUri`
  for outbound quoted Notes; the quote serializer regression now covers all
  three identifiers.
- Added deterministic simulated quota-failure coverage for Paperclip media.
  A test-support storage-full fault after the original file write proves that
  derivative failure removes every partial file, preserves the unmaterialized
  database row, and allows a durable remote-media retry to succeed. The focused
  Paperclip test and restored worker integration pass; real filesystem quota
  exhaustion remains external hardening evidence.
- Closed the final inbound remote `Flag` fixture gaps: local collection
  resolution now accepts Mastodon's numeric `/collections/:id` web form, and
  teardown removes report-linked notifications, notification jobs, mail jobs,
  durable jobs, and immutable stream-event outbox rows for every test Flag URI.
  The restored worker fixture passes 43/43 and the full `mise run check` gate is
  green; separate `LOCAL_DOMAIN`/`WEB_DOMAIN` route matching remains a broader
  parity follow-up.
- Hardened signed ActivityPub POST transport with deterministic fixtures for
  mixed DNS answers, redirect-hop rebinding, same-origin 307/308 follow-up,
  302 rejection, timeout, and oversized responses. The full `mise run check`
  gate is green, including Clippy, formatting, all-target tests, dependency
  policy, and pinned-fixture verification; live Mastodon peer convergence
  remains external acceptance work.
- Extended the restored ActivityPub relationship worker matrix with duplicate
  Accept, Reject, Block, and Undo Block deliveries. The database relationship
  state converges without duplicate rows after replay, and
  `mise run worker-integration` passes 43/43; live peer and cross-instance
  ordering evidence remain open.
- Added a 32-address ceiling to remote DNS answer collection before SSRF
  policy validation, with a boundary unit test. This closes the remaining local
  resolver-cardinality hardening gap without changing the public address policy.
- Matched Mastodon's outbound authorization-failure behavior: a `401` delivery
  response is permanent only for a source account suspended without an account
  deletion request; active and temporarily suspended accounts retry. The
  delivery classifier regression covers both branches.
- Re-ran the complete local acceptance gates: differential compatibility passed
  20 phases, Mastodon schema passed 35/35, operational schema and streaming,
  startup safety, preflight, cutover/rollback, worker integration 43/43, and
  the full `mise run check` all passed. Live peer, browser/mobile recording,
  quota, sustained-load, and hard-power-loss evidence remain open.

## 2026-09-06

- Extended v1 hardening with a restored PostgreSQL acknowledgement-failure
  regression, a twenty-job/four-permit idempotency burst, and Paperclip
  derivative-failure rollback coverage. Worker integration now passes 42/42;
  quota exhaustion, sustained load, hard power-loss, and production reopen
  evidence remain external or future hardening work.
- Added relationship-specific outbound ordering coverage. A real signed Follow
   delivery is held open while the write path records its Undo successor; a
   second Push worker cannot claim the successor, and the released fixture sees
   Follow before Undo. Live peer convergence remains open.
- Added the matching live-lease Block/Undo ordering regression. The restored
   worker fixture holds a signed Block delivery open, records its Undo successor,
   fences a competing Push claim, and verifies Block before Undo on the wire.
   Worker integration now passes 43/43; live peer convergence remains open.
- Corrected suspension-origin handling against Mastodon 4.6.5: local moderation
  now has restored-fixture proof for remote-follow teardown, notification and
  counter cleanup, pending Accept cancellation, and durable Reject intent, while
  remote-origin actor `Update` suspension remains non-destructive to follows.
- Aligned outbound Reject identity with Mastodon 4.6.5. Persisted Follow and
  FollowRequest rows now use their numeric row IDs across blocking, follower
  removal, request rejection, actor deletion, and suspension; an immediate
  blocked Follow keeps Mastodon's empty no-row suffix. Unit, schema, and worker
  coverage pass, while live peer convergence remains open.
- Closed the basic moderation and reconciliation issue after the restored
   moderation lifecycle, domain purge, counter repair, rollback, differential,
   and suspension-side-effect gates passed. Live peer and production evidence
   remain tracked by the broader acceptance and hardening issues.
- Extended inbound remote `Flag` reports to retain local target status IDs and
   collection relationships, matching the pinned Mastodon 4.6.5 ActivityPub
   handler's visibility and mention rules for local targets. One Flag now
   creates a report per local target account, with canonical account/domain
   locks around suspension and `reject_reports` policy checks; generated and
   OStatus tag-style object IDs are resolved while unsupported report IDs are
   filtered. The
   restored worker fixture proves account, public/private/direct status,
   collection, multi-target, suspended-reporter, and domain-rejection
   behavior; remote-target reply reporting remains outside this local-target
   slice.
- Closed the status-social-interactions issue after rerunning the current gates:
  restored Mastodon schema integration passes 35/35, worker integration passes
  43/43, and the full differential workflow passes 18 general cases plus the
  notification and status-authorization phases. Interaction routes, counters,
  idempotency, locking, notification, and outbound lifecycle behavior are now
  locally evidenced; live peer convergence remains external.
- Closed the account-relationship-writes issue after the same current gates
  proved duplicate-safe follow, follow-request, block, mute, counter,
  notification, expiry, and local outbound-intent behavior. Remote peer
  convergence remains with the ActivityPub federation issues.
- Closed the write-foundation issue after the current differential write matrix,
  restored 35/35 Mastodon schema tests, cutover reopen rehearsal, and narrow
  typed writer boundary verified the transaction, locking, idempotency, outbox,
  and database/media contracts. Feature-specific and live-peer work remains
  with its owning issues.
- The complete Rails-versus-Rust differential workflow passes 18 general cases
  plus isolated notification-write and status-authorization phases (20 test
  phases total) against Mastodon 4.6.5. The earlier ten-minute wrapper timeout
  was command-duration related; `rest_protocol_contracts` passes in isolation
  and the full workflow completes with a longer timeout.
- Fixed cutover web-process cleanup by making `run_web_cli()` replace its
  intermediate shell with Rustodon via `exec`; rerunning
  `mise run cutover-integration` passed and left no Rustodon web listeners
  after cleanup. Stale listeners from earlier rehearsals were removed.
- Completed the browser TOTP/recovery-code management slice. Rust-owned setup,
  confirmation, regeneration, and disable routes now use Active Record encrypted
  OTP secrets, bcrypt recovery-code hashes, password challenges, required-role
  protection, and transactional WebAuthn cleanup. Guarded Mastodon 4.6.5 cases
  prove the encrypted production repository path, rate-limit boundary, role
  protection, recovery lifecycle, and database restoration; live browser/client
  evidence remains open.
- Added representative Mastodon-owned preservation rows for an unresolved
  account migration, announcements, appeals, backups, a failed bulk-import row,
  report notes, and a valid Web Push subscription. SQL and Rails verification
  assert their values and foreign-key links; the 35-case schema gate, complete
  19-case differential suite, and cutover/rollback rehearsal all prove the rows
  survive Rustodon startup and Mastodon reopen. `PRESERVE-06` is now locally
  automated; production and client acceptance remain open.
- Extended the cutover rehearsal with a fresh JPEG upload through Rustodon. The
  isolated media root is reopened by pinned Mastodon 4.6.5, where Rails metadata,
  the v1 media response, and original/small Paperclip files are verified before
  restoring the database row, sequence, and filesystem baseline. The old Redis
  container is removed before Mastodon is restored with a fresh empty Redis
  instance. Paperclip file creation now explicitly applies `0644` after
  restrictive process umasks so a reopened Mastodon process running as another
  user can read new media. The cutover gate passes.
- Extended the browser account-settings differential case with an isolated
  moderator profile upload. Multipart avatar and header submissions now prove
  redirect behavior, descriptions, JPEG metadata, generated Paperclip files,
  and database/media cleanup back to the baseline. The focused case passes
  against the restored Mastodon 4.6.5 fixture; client/mobile acceptance remains
  open.
- Extended the authenticated WebSocket fixture with deterministic reconnect
  coverage. After the initial batch is consumed, a post-handshake event is
  delivered exactly once on the new connection and earlier events are not
  replayed. Operational-schema integration passes; full pinned-client filtering
  and live peer convergence remain open.
- Fixed a profile-settings media regression: saving profile fields without an
  avatar or header upload no longer deletes the existing Paperclip files. The
  account-settings differential case now snapshots the Rust media tree to keep
  this invariant covered. The complete Rails-versus-Rust workflow passes 19/19
  cases against Mastodon 4.6.5 (17 general plus notification and status phases).

## 2026-09-03

- Completed the account-lifecycle write-fencing pass. Authenticated local write
  entry points now recheck lifecycle state inside their transactions through the
  shared `begin_account_write` helper; profile/media filesystem work remains
  under the canonical account lock. Actor-delete delivery rechecks state while
  holding that lock through the final send, and account purge repairs accepted
  quote counters when source statuses disappear. The restored worker gate passes
  39/39, the Mastodon schema gate passes 35/35, and `mise run check` is green.
  The stale-authentication regression now covers every identified authenticated
  write entry point; hard power-loss compensation remains open.
- Advanced v1 release hardening with a deterministic all-lane worker crash
  regression. An aborted leased handler is reclaimed and acknowledged exactly
  once across Ingress, Core, Push, Pull, Mail, and Maintenance. Worker
  integration now passes 40/40; disk-full, PostgreSQL outage, sustained-load,
  power-loss, and live-peer reopen evidence remain open.
- Added the executable `mise run cutover-integration` rehearsal. It migrates
  Rustodon's isolated operational schema, starts the least-privilege worker and
  web processes, runs health/auth/read/write/media/WebFinger/ActivityPub smoke,
  stops Rustodon, reopens the pinned Mastodon web process, verifies Rails
  reads, and proves stable public catalog/schema/data/auth state and Paperclip
  media are preserved without reversing Mastodon migrations. The gate passes.
- Hardened `tools/rustodon-smoke` for local reverse-proxy fixtures: it sends the
  configured host, uses native HEAD requests, forwards optional trusted HTTPS
  protocol metadata, forwards public-request curl options, and keeps bearer
  tokens out of curl process arguments via a temporary mode-restricted header
  file.
- Extended the restored moderation worker proof for administrative domain purge:
  case-normalized exact-domain selection preserves subdomains, removes remote
  account/status/report/notification descendants and Paperclip files, refreshes
  instances, preserves local counters and severance type boundaries, emits no
  stream effects, and safely replays after completion. Worker integration passes
  34/34; crash-time filesystem compensation and live peer evidence remain open.
- Added a restored-fixture remote-media lease-fence regression: a blocked fetch is
  fenced after its durable lease expires, leaves the attachment and job
  recoverable, and a recovery invocation materializes the original and small
  derivatives without a dead letter. Worker integration passes 35/35; concurrent
  stale writes, ambiguous metadata commits, and shutdown-specific media
  cancellation remain open.
- Added route-aware CORS preflight and response headers for OAuth token/revoke,
  userinfo, discovery, NodeInfo, and public account endpoints. OAuth browser
  sign-in now preserves a validated local authorization return target, and the
  authorization-code differential case plus focused browser tests pass.
- Replaced the process-local remote signature-refresh cooldown with a shared
  PostgreSQL marker in `rustodon.rate_limit_windows` when a web pool is
  configured, retaining the local fallback for pool-less state. Transport/DNS/
  client/body-read failures still trip the five-minute marker, while ordinary
  remote HTTP errors do not. Independent-pool visibility, client isolation, and
  expiry cleanup pass in the operational fixture.
- Added expired `rate_limit_windows` pruning to the durable maintenance handler;
  the least-privileged runtime worker integration now proves marker cleanup
  alongside the existing operational maintenance checks.
- Post-CORS and shared-circuit verification passes `mise run check` (193 passed,
  2 ignored), `mise run operational-schema-integration`, formatting, diff, and
  Clippy checks. The direct `cargo deny` command is unavailable outside Mise;
  Mise's pinned dependency-audit task passes.
- Added operational-schema migration 3 with expiring per-canonical-host remote
  fetch leases. Web and worker fetchers now coordinate the two-request budget
  across independent pools through transaction-locked PostgreSQL rows, release
  leases on success/error/cancellation, reclaim expired rows during acquisition
  and maintenance, and retain the process-local fallback for pool-less tests.
- Added least-privilege runtime grants, schema catalog/upgrade coverage, and
  independent-pool lease tests. The local gate passes 193 tests with two ignored,
  operational-schema integration passes, worker integration passes 34/34, and
  startup integration passes 4/4 against Mastodon 4.6.5/PostgreSQL 14.23.
- Added transactional grants for newly introduced operational tables during
  upgrades, so an existing v2 runtime role can migrate to schema v3 without
  failing before the new remote-fetch lease privilege is available. The
  fixture-backed v2-to-v3 upgrade regression passes.
- Preflight now validates an existing Rustodon operational schema and runtime
  role, while allowing the documented first run before operational migration.
  The preflight fixture proves both the valid path and a revoked lease grant.
- Added `docs/mastodon-writer-grants.sql`, an executable least-privilege writer
  ACL recipe covering Mastodon and Rustodon tables, columns, sequences,
  functions, and rejected `PUBLIC` grants. The restored startup fixture now
  executes the same recipe.

## 2026-09-02

- Completed the least-privilege writer ACL review for remote media, quotes,
  tombstones, and account-deletion request row locks. Preflight now requires
  `domain_allows`, `quotes`, tombstone table/sequence capabilities, and
  `UPDATE` on `account_deletion_requests` for its `FOR UPDATE` path; the
  restored fixture grants exactly those capabilities. Startup coverage rejects
  revoked tombstone access, an unexpected quote insert grant, and arbitrary
  column-level `SELECT` or `REFERENCES` grants.
- Re-ran the restored-fixture gates after the ACL remediation: schema 35/35,
  worker 34/34, startup 4/4, and all 19 Rails-versus-Rust differential cases
  passed against Mastodon 4.6.5 (17 general plus notification and status
  authorization phases) in 724.31 seconds.
- Extended the restored WebSocket fixture coverage so an unauthorized private
  status update is suppressed before an authorized public update is delivered.
  Operational schema integration passes, including both streaming tests.
- Isolated stateful differential phases by restarting Rails/Redis and
  re-precomputing feeds before notification and status authorization cases.
  The complete differential workflow passes 17 general cases plus the two
  isolated cases.
- Completed the PostgreSQL 14 writer-ACL hardening. Startup validation now
  checks default `CREATE` privileges, explicit public ACL drift on types,
  languages, foreign objects, and tablespaces, large-object settings and ACLs,
  and ownership across the supported PostgreSQL catalog object classes while
  preserving PostgreSQL's built-in default ACLs.
- Expanded guarded startup mutations for default schema privileges, public
  foreign-object and tablespace grants, catalog objects, large objects, and
  role settings. The restored production startup safety integration passes all
  3/3 tests against the pinned Mastodon 4.6.5/PostgreSQL 14.23 fixture, and
  `mise run check` passes with 191 unit tests and one ignored test.

## 2026-09-01

- Completed the read-only writer-pool and advisory-lock review against the pinned
  Mastodon 4.6.5 checkout. Found shared-pool self-starvation when lock callbacks
  acquire nested connections, a web/admin `DB_POOL` sizing mismatch, and a
  single-label domain-block lock-scope gap. Startup, worker, and preflight
  integration gates pass after restoring the fixture function grant in the
  unsafe-role test; remediation remains tracked in the open review issue.
- Remediated the reviewed resource and race findings: lock ownership uses a
  dedicated PostgreSQL connection, configured writer pool sizes reach web and
  admin constructors, remote actor upserts recheck policy inside their lock,
  and remote media retries can reclaim stale `processing = 1` claims.
- Remote media cleanup now removes staged files on cancellation before commit,
  retains files across ambiguous commits for retry reconciliation, and refuses
  stale error paths from downgrading rows that already have a file. The seeded
  stale-claim worker regression and the full restored worker suite pass 34/34.
- Remote media finalization now evaluates the typed global-domain `reject_media`
  policy in its write transaction while the dedicated advisory-lock transaction
  remains held. A restored worker regression proves a domain-blocked attachment
  is failed closed without changing its parent status.
- A fresh full Rails-versus-Rust differential run passed all 19 cases against
  Mastodon 4.6.5 in 681.25 seconds. The earlier ten-minute wrapper timeout was
  isolated to setup/runtime duration; the individual OAuth case also passed.
- Final verification passes the 192 library tests (191 passed, 1 ignored), all
  non-ignored integration targets, lint, dependency audit, startup 3/3, schema
  35/35, preflight, worker 34/34, operational-schema, formatting, and diff
  checks. Actual lease-fence and ambiguous-commit fault injection remain open.
- Favourite and bookmark removal now follows Rails' association-first behavior
  after an author block, including the unauthorized response projection. Remote
  favourite deletion is no longer blocked by creation-time domain policy, so
  existing rows can be removed and Undo delivery recorded. The guarded
  interaction differential and all 35 restored-fixture schema tests pass.
- Global domain-block side effects now cross a durable boundary. Suspend-level
  and reject-media updates enqueue a transactional maintenance job; the worker
  rechecks the block generation and account suspension timestamp before applying
  cleanup, so retries cannot purge a newly changed account state.
- Domain suspension now records Rails-compatible relationship severance events,
  preserves active/passive follow settings, creates local severance notifications
  idempotently, clears matching Paperclip metadata, removes configured files
  without following symlinks, and deletes remote custom emoji rows. Restored
  Mastodon schema and worker coverage pass, including a filesystem-backed case.
- Remote actor `Update` activities now validate and persist Mastodon's
  `suspended` state with remote suspension origin, allow remote unsuspension,
  and keep local suspensions and deleted-account tombstones fenced. Remote Note
  and actor deletion plus due account purge now collect Paperclip metadata before
  safely removing files; reported account content remains protected. The full
  worker integration suite passes 32/32.
- Added the least-privilege writer contract for severance tables and sequences,
  and removed row locks from the `poll_votes` read that the writer role cannot
  legally acquire. The remaining gaps are full admin domain purge semantics,
  ancillary remote-suspension side effects, immediate streaming disconnects,
  and crash-time filesystem/database compensation.
- Writer preflight now permits exactly the severance updates and shared
  `rate_limit_windows` operations used by the web writer pool; the fixture ACL
  contract and production startup safety integration pass. Remote media fetch
  completion also rechecks its parent status is still live before committing
  metadata, cleaning staged files when a concurrent deletion wins.
- ActivityPub inbox ordering and logical activity identity now use the verified
  actor URI rather than a signing key ID. Retained fingerprints reject
  conflicting retries with `409 Conflict`; a restored-fixture HTTP regression
  proves key-rotation ordering, idempotent retries, and conflict rejection.
- Explicitly ran the full write-transaction differential case after the
  association-first saved-interaction changes; the blocked bookmark/favourite
  removal path passes against Mastodon 4.6.5. The restored durable-worker suite
  passes 33/33, including signed delivery, transient retry, crash replay, and
  same-inbox cross-worker ordering for outbound ActivityPub work. Live peer
  convergence remains the only unverified delivery boundary.
- Embedded remote `Undo Follow` and `Undo Block` writes now fence deletion by
  the referenced relationship URI, preventing an older undo from removing a
  newer relationship for the same actor pair. Restored-fixture coverage proves
  stale and matching embedded undos for both relationship types.
- Local suspension and self-service account deletion now record an atomic
  `kill` stream event. WebSocket connections poll system events without waiting
  for a subscription and close with Mastodon's normal code; a second
  post-cursor authentication closes the suspension race. The operational fixture
  proves the live suspension path and preserves public schema/data immutability.

## 2026-08-31

- Added the Rails `throttle_api_media` equivalent for authenticated `POST`
  requests to both media API versions: 30 requests per user in 30 minutes.
  User identity is resolved without requiring write scope before the normal
  media authorization path, matching Rack::Attack's treatment of insufficient-
  scope bearer tokens. Production web state uses a transactional PostgreSQL
  window, so independent pools share the bucket and database failures fail
  closed. Unit, cross-pool, and restored differential media coverage prove the
  boundary and rate-limited response behavior without leaving media artifacts.
- Hardened credential-sensitive cache boundaries. ActivityPub status responses
  now vary on `Authorization` and `Signature` even for anonymous cache entries,
  and requests carrying viewer credentials receive `private, no-store` instead
  of entering a shared cache. Status-authorized Paperclip media keeps public
  anonymous caching only with credential-aware variation; authenticated local
  media and cached remote media are private/no-store. Unit, federation, local
  Paperclip, and discarded-media differential coverage prove the policy.
- Separated browser authentication from functional account access to match the
  pinned Rails lifecycle. Disabled, suspended, and moved non-memorial accounts
  can authenticate and retain a usable Rust-owned browser session; memorial
  accounts remain rejected. The differential browser-authentication case covers
  all four lifecycle states. Browser sessions now carry functional state so
  non-functional accounts cannot reach OAuth consent, while OAuth/API
  authorization remains fail-closed.
- Administrative local suspensions now enqueue the existing durable account
  purge job for 30 days after the deletion request, transactionally with the
  moderation state, plus a same-time durable ActivityPub actor-delete intent.
  Restored Mastodon schema coverage verifies both scheduled payloads and that
  unsuspension cancels them.
- Added the `rate_limit_windows` CRUD privileges to the operational-schema
  runtime ACL allowlist. The worker integration now migrates twice and passes
  all 30 durable-worker cases with the least-privilege role.

## 2026-08-29

- Account status pages now match Rails' `reblogs_may_occur?` behavior: tagged
  and media-filtered requests do not apply source-account block, mute, or
  domain-block filters. Restored-fixture coverage proves a tagged reblog is
  retained when its source is blocked.
- ActivityPub viewer authentication is now route-specific: status documents
  retain browser/OAuth access, while outbox and replies/likes/shares collections
  use signed-request identity only. Private collection responses reject browser
  and bearer viewers, and signed responses are marked private/no-store.
- Follow-request authorization now emits an ActivityPub `Accept` only for
  remote accounts whose protocol is ActivityPub, matching Rails' source-account
  check. OStatus requests still become local follows without outbound Accept
  delivery; restored-fixture coverage proves both paths.
- ActivityPub inbox requests now have a process-local 300-per-five-minute
  trusted-client-IP limit before body buffering or signature-key resolution.
  IPv6 clients share their `/64` bucket and the limiter returns Mastodon-style
  rate-limit headers; the boundary is covered by a unit regression.
- Bounded request bodies now have a 30-second read deadline in addition to
  their byte limits, preventing stalled clients from holding REST or inbox
  workers indefinitely. A shared process-local remote-fetch budget also caps
  each canonical remote host at two simultaneous GET/POST/DNS operations;
  contention retries without tripping signature circuits or domain health, and
  idle buckets are evicted at the bounded host-state ceiling. SSRF validation
  also normalizes IPv4-compatible IPv6 addresses through the IPv4 policy.
- Large bodies on required API routes are now admitted only after valid bearer
  header or query credentials are checked. Requests without those credentials
  and public routes use the 4 MiB Rack parameter ceiling instead of reserving
  the 99 MiB REST buffer before authentication; browser profile uploads require
  a valid session before retaining their 12 MiB route limit.
- Account-update reach now uses Rails' `suspended_at - 2 days` cutoff for locally
  suspended accounts instead of the wall-clock cutoff. Restored worker coverage
  proves a delayed actor update still reaches a recently followed remote account.
- Status updates now apply nested Mastodon `media_attributes[]` descriptions and
  image focus values to retained attachments transactionally. The status edit
  path now creates the initial previous snapshot only once, matching Rails on
  repeated edits. Schema and differential coverage pass, including REST response
  metadata and edit-history behavior.
- Status media IDs now preserve Rails' first-occurrence order without duplicates,
  historical edit media is capped at four attachments, and edit timestamps use
  the persistence time for `updated_at`. Video thumbnail replacement remains
  outside the image-only media scope.
- Remote URI-only Accept/Reject decisions now require an exact follow URI and
  cannot consume NULL-URI relationship rows. Restored-fixture worker coverage
  proves mismatched decisions are no-ops while matching decisions remain valid;
  worker integration passes 29/29.
- Corrected global-domain severity handling: nullable and unknown existing
  rules remain fail-closed in policy and can be repaired without weakening
  them. Local report creation now matches Mastodon `ReportService` and is not
  rejected based on the target domain; `reject_reports` remains an inbound
  remote `Flag` concern, outside the current v1 scope. Schema integration
  passes 30/30.
- Added a process-local Mastodon-compatible `media_proxy` abuse throttle: 30
  requests per trusted client IP in 10 minutes, with the standard `429`
  rate-limit headers. The limiter runs before media lookup or remote fetching;
  the unit boundary test proves requests 1-30 pass and request 31 is denied.
- Status language values now follow the pinned Rails locale cascade: supported
  regional locales are preserved, unsupported variants such as `fr-FR` fall
  back to `fr`, and unknown values fall back to the current/default language.
  The guarded differential write case proves the persisted edit state.
- Notification-request merge status now reflects pending transactional unfilter
  work in either the operational outbox or durable-job queue instead of always
  returning `merged: true`. Worker integration proves outbox-pending,
  durable-job-pending, and completed states.
- Single notification-request dismissal now records a per-request transactional
  cleanup job. The Core worker deletes that sender's filtered notifications in
  bounded batches; bulk dismissal retains the pinned Rails direct-destroy
  behavior. Worker coverage proves delayed cleanup and repeated dismissals.
- Unknown ActivityPub Note Updates older than 24 hours are now ignored before
 materialization, matching the pinned Rails Update path. Known statuses and
 tombstones remain unaffected; unit and worker regressions cover the fence.
- Canonical ActivityPub Note reads and local outbox pages now apply the shared
  status authorization policy to signed/OAuth viewers. Private, direct, and
  limited content is exposed only to the correct audience, while author blocks
  and domain blocks remain fail-closed; the federation differential case covers
  anonymous, follower, mentioned, blocked, username, numeric, and paginated
  routes.
- Added username and numeric ActivityPub status collection routes for `replies`,
  `likes`, and `shares`. Standalone replies serialize local Notes but preserve
  remote URI items, follow Rails' self-reply/other-account pagination, and apply
  parent status authorization before returning any collection or count.
- Matched Rails boost routing by redirecting status-object GETs to the original
  status and keeping boost collection URLs off the `/activity` object URI. Note
  and activity responses now include the exact alternate ActivityPub `Link`
  header; the guarded federation case checks redirects, collection IDs, and
  status-document headers.
- Matched Rails status response caching while hardening shared-cache isolation:
  non-credentialed distributable status documents use `max-age=180, public`,
  pending quotes use five seconds, and every status entry also varies on
  `Authorization` and `Signature`; credentialed status responses are private
  and not stored. Differential coverage compares the Rails baseline while
  explicitly allowing these security-only cache additions.
- Applied the same Rails `Vary` and `private, no-store` defaults to status-route
  errors and local boost redirects, and extended the federation differential
  checks to cover non-200 status responses as well as successful documents.
- ActivityPub status and collection reads now honor valid browser session cookies
  before signed or bearer viewers, matching Rails web-session precedence; limited
  federation still requires a valid request signature. Optional invalid
  signatures continue to fall back to anonymous public-fetch reads.
- Added Rails-compatible `inReplyToAtomUri`, `conversation`, and `context` Note
  fields for federation reads and outbound worker Notes, including generated
  local OStatus reply identifiers when a parent has no stored URI.
- Matched Rails' five-second public cache window for distributable ActivityPub
  statuses with pending quotes. The status and activity routes query pending
  quote state before applying the shared response cache policy, while Activity
  objects retain their normal three-minute cache.
- Quote listing now filters both directions of account blocks before applying
  pagination, matching Rails and preventing hidden quote authors from producing
  stale cursors. Schema integration passes 31/31 with the regression case.
- Account-only remote ActivityPub `Flag` reports now run through the durable
  inbox worker. Comments are capped at Mastodon's 5,000-character limit,
  suspended reporters are ignored, report staff notifications and configured
  mail jobs are queued transactionally, and the longest matching domain rule
  can reject reports. Hostname policy matching now ignores remote ports while
  origin validation retains them. Retroactive account restrictions normalize
  stored port-bearing domains, and inbox actor domains strip explicit default
  ports while preserving non-default ports. Restored-fixture worker integration
  passes 30/30; status, collection, and remote-target Flag objects remain
  deferred.
- Account timeline reblog-source filtering now bypasses blocks, mutes, and
  domain blocks for the timeline owner, matching Rails' `AccountStatusesFilter`.
  Restored schema integration covers the owner-visible boost regression.
- Hashtag timelines retain all tagged public languages for authenticated viewers,
  matching Rails' `TagFeed#get` override, which does not call `PublicFeed`'s
  chosen-language scope. Restored schema and differential coverage preserve the
  anonymous-versus-authenticated result.
- Status notification fan-out now honors Mastodon's `USER_ACTIVE_DAYS` setting
  instead of hard-coding the default seven-day activity window. Missing values
  default to seven days and invalid numeric values convert to zero like Ruby's
  `to_i` boundary.

## 2026-08-28

- Reconciled outbound ActivityPub reach with the pinned Mastodon 4.6.5
  `StatusReachFinder` and `AccountReachFinder`: suspension-triggered actor
  updates are retained and delivered, same-second account updates have
  microsecond-versioned delivery keys, recent account reach caps are applied
  after preferred-inbox grouping, and alternate-host inboxes remain valid
  bounded delivery targets. Remote Likes retain direct actor inbox delivery;
  Announce and Undo Announce use the preferred shared inbox so status fan-out
  deduplication cannot discard a boost. The final local gates pass: 172
  library tests plus all target binaries, 29/29 worker tests, 30/30 schema
  tests, and all 19 differential cases. Live peer convergence, client
  acceptance, and cross-worker ordering remain open.
- Corrected outbound status reach for remote quotes by joining
  `quotes.quoted_status_id` rather than the quoting status ID. A restored
  worker regression isolates a remote quoter on a unique inbox and proves
  edited status delivery; worker integration passes 29/29. Live peer
  convergence and cross-worker ordering remain open.
- Added the executable v1 acceptance matrix and required-route inventory test,
  then ran the complete 18-case Rails-versus-Rust differential suite against
  the pinned Mastodon 4.6.5 fixture; every case passed. Browser/mobile, live
  peer, policy cross-surface, and cutover evidence remain explicitly open.
- Added a bounded process-local five-minute remote signature-key refresh circuit
  keyed by trusted client IP. Only transport/DNS/client/body-read failures trip
  it, and signature-dependent ActivityPub status responses now vary on
  `Authorization` and `Signature` with private caching for authenticated fetches.
-  Actor, Note, activity, outbox, and follower/following collection reads now
  enforce the same signature policy in limited federation mode while `/actor`
  remains public. Web and authorized-fetch coverage, federation differential
  coverage, full tests, lint, format, dependency audit, and fixture verification
  pass.
- Added the Rust-owned self-service `/settings/delete` flow with CSRF and
  password/username confirmation. A successful request atomically marks the
  local account unavailable, records Mastodon's deletion request, cancels
  pending actor updates, signs out the browser session, and queues a durable
  ActivityPub actor `Delete`. The Push worker deduplicates remote/shared
  inboxes and relays, applies federation policy, and retains the signing key
  needed by the signed delivery. Full account-content purge and relationship
  severance remain deferred. The new worker regression passes; the complete
  18-case differential suite passes.
- Browser authentication and recovery failures now negotiate HTML for browser
  form submissions instead of returning JSON into a navigation. Login errors
  preserve the submitted email and CSRF state, reset-password errors preserve
  the reset form/token, confirmation failures provide a safe return link, and
  non-HTML clients retain the existing JSON envelopes. The guarded browser
  authentication and federation cases pass against the pinned Mastodon 4.6.5
  fixture.
- Remote signed reply Creates now retain their original activity JSON and are
  durably forwarded to the local reply parent's remote followers through the
  parent account's signer, excluding the sender inbox and preferring shared
  inboxes. Forwarding keys are immutable per activity/inbox, and replaying a
 Create does not reset a dispatched delivery.
- Remote signed Note Creates now also forward through local reblogger and
  quoter followers, while signed Note Updates and Deletes retain their
  activities and use the same durable forwarding path. Restored-fixture worker
  coverage proves reply, reblog, quote, update, and delete forwarding.
- Deleting a remote original Note now records durable `Delete` intents for
  affected local reblog wrappers, allowing their `Undo Announce` activities to
  reach remote followers. Local original-status deletion uses the same
  transactional path; worker and schema integration remain green.
- Narrowed deleted-status reach to match Mastodon: `include_unsafe` still
  preserves historical interaction recipients, but deleted direct or limited
  replies no longer reach the reply target or that target's followers. A
  restored worker regression covers the privacy boundary; worker integration
  passes 29/29.
- Local reply reach now includes remote followers of the local parent author,
  with protocol, suspension, and domain-block filtering. Actor-delete coverage
  also proves deleted-actor and affected-local account counters return to the
  expected values. Worker integration passes 26/26; the aggregate local check
  passes.

## 2026-08-27

- Remote Notes addressed to specific accounts now persist with Mastodon's
  `limited` visibility (`4`) instead of being misclassified as direct; local
  direct posts remain `3`. Deleted statuses now unlink their IDs from account
  conversations, and block or notification-hiding mute writes share atomic
  cleanup for conversations, notifications, and notification requests. Focused
  coverage passes schema integration `30/30` and worker integration `26/26`.
- Added Mastodon-compatible streaming endpoint aliases for `user`,
  `user:notification`, and `direct`, including path-selected initial streams.
  Notification status updates now reach both legacy `user` and dedicated
  notification subscriptions when scopes allow it. Remote audience URIs now
  resolve through canonical local ActivityPub aliases, so silent limited
  mentions receive user-stream create/update/delete events; status deletion
  fan-out also covers all current local followers instead of reapplying live
  feed filters. Operational WebSocket, schema, and worker integration remain
  green.
- The full Rust target matrix and strict Clippy pass under the pinned Rust
  `1.97.1` toolchain. The unprefixed commands still select installed Rust
  `1.97.0`, which Cargo rejects per the repository's `rust-version` requirement.
- Completed the account, media, and authentication release-safety review. Locked
  account visibility defaults now fail private, status creation honors explicit
  and stored quote policies, and private browser posting defaults normalize quote
  policy to `nobody`. The full local check and all 18 guarded differential cases
  pass against the pinned Mastodon 4.6.5 fixture.
- Push delivery now derives a stable source-account/inbox ordering key. Pending
  outbox events dispatch in order, ordered jobs retain predecessor IDs, claims
  fence successors during live or recovering deliveries, and marker cleanup
  preserves active chains. Concurrent first-marker creation, expired-marker
  recovery, and abandoned-lease ordering now pass in the 24-test worker suite;
  live peer convergence and wire-level crash/retry proof remain open.
- Added durable-worker coverage for a real transient signed delivery retry:
  `503` records domain health and leaves the job durable, cooldown recovery
  permits the next attempt, and `202` clears both the job and failure state.
  Restored-fixture worker integration now passes 25/25; process-crash duplicate
  delivery and live peer convergence remain open.
- Added transport-boundary crash/replay coverage: a local peer accepts a signed
  POST before the worker is aborted, lease recovery replays the activity, and an
  ordered successor remains fenced until replay completes. Worker integration
  now passes 26/26; peer-side idempotency and live Mastodon convergence remain
  open.
- Corrected authenticated user-stream fan-out for limited and direct statuses.
  The pinned Mastodon behavior delivers these statuses to local followers who
  are explicitly mentioned; Rustodon now removes the contradictory public-only
  filter and proves the author/follower recipients with a restored-fixture
  regression. Schema integration passes 27/27, worker integration 26/26, and
  all 18 guarded differential cases still pass.
- Reblog create and removal now record transactional authenticated user-stream
  `update`/`delete` events. Follower fan-out honors `show_reblogs` while keeping
  the booster event and existing block/mute policy checks; the restored schema
  suite passes 28/28 and the full quality, worker, and differential gates pass.
- Stream-event writes, cursor snapshots, and reads now share a transaction-level
  PostgreSQL advisory lock, preventing pre-commit identity gaps from making a
  later committed event advance a client past an earlier one. A concurrent
  commit-order regression covers the race; operational integration passes with
  the WebSocket replay test.
- Original-status deletion now records user-stream `delete` events for every
  soft-deleted reblog wrapper before the original event. The restored schema
  regression proves wrapper cleanup for all local followers; schema integration
  now passes 29/29.
- Incoming remote Note Create/Update/Delete and Announce/Undo writes now record
  transactional authenticated user-stream `update`/`delete` events, including
  remote reblog wrappers removed by Note Delete and URI-only Undo handling.
  Restored worker coverage proves the remote lifecycle; schema integration
  passes 29/29, operational stream/WebSocket checks pass, `mise run check`
  passes 159 tests, and all 18 guarded differential cases pass. Full client
  stream filtering and live peer convergence remain open.
- Authenticated conversation events now use Mastodon's dedicated `direct`
  stream, with `read:statuses` scope enforcement and no fallback to `user`.
  User-stream status fan-out now applies the pinned home-feed language, reply,
  mute/block, domain-block, exclusive-list, mention, and reblog-author filters.
  Expiring mutes are honored consistently for reblog sources and blocked
  mentions. Mention-only status updates now emit a private internal stream
  marker for the recipient's notification stream. Restored schema and worker
  integration pass 29/29 and 26/26 respectively.
- Added the Rust-owned minimal account settings surface for profile, appearance,
  posting defaults, security, password changes, and authenticated browser session
  flows with escaped HTML and CSRF protection. Browser settings coverage proves
  persistence and rejection paths; successful browser media upload and 2FA
  management remain outside the current proof.
- Local Paperclip media and thumbnail routes now apply status audience policy
  before opening files, deny unattached/deleted media to ordinary viewers, and
  preserve Mastodon's `manage_reports` exception for discarded media. The
  authorization decision has pure unit coverage and the guarded
  `local_paperclip_deleted_media` differential case proves anonymous denial and
  moderator access after soft deletion.
- The pinned Mastodon 4.6.5 production frontend is now packaged under
  `public`, recorded with its source revision/build contract and a generated
  `SHA256SUMS` file. Rustodon serves the hashed Vite assets, PWA manifest,
  service worker, public notification assets, favicon, and SPA shell with
  escaped initial state, CSRF/VAPID metadata, authenticated session hydration,
  and deep-link fallback. The shell now covers the complete pinned web-app route
  inventory, applies Rails-equivalent HTML security headers with a nonce-backed
  CSP, rejects revoked or expired browser-session tokens, and restricts remote
  media proxy responses to Mastodon's supported media MIME types. Unit coverage
  validates the manifest and shell contract; guarded browser-auth and
  `local_web_client_shell` cases cover the live router paths.
- Completed the unknown remote Announce path through the durable Pull lane.
  Embedded self-boost Notes are persisted without a fetch; unknown targets use
  bounded same-origin-checked fetches, local-follower signatures, remote actor
  upserts, and tombstone/relevance fencing. Fetched Create wrappers preserve
  their activity URI separately from the inner Note URI, and nested Announce
  targets resolve recursively up to a bounded depth.
- Remote Note persistence now records Mastodon's ordered media attachment IDs,
  retains visibility across Update payloads, ignores content replacement when
  no explicit edit timestamp is supplied, fences media work after deletion, and
  removes dependent status/mention/quote-update notifications on delete.
- Restored-fixture worker coverage passes 22/22. `mise run check` passes with
  155 library/target tests, strict Clippy, dependency checks, and fixture
  verification. Live peer convergence and the full differential/adversarial
  federation matrix remain open.

## 2026-08-26

- Closed the remaining Rails-versus-Rust status interaction mismatches in the
  guarded fixture: duplicate and generated-ID unreblogs now preserve literal
  request semantics, remove owned boosts even when the source is no longer
  readable, maintain status/account counters, and serialize the correct
  response branch. Reblog creation now rejects both directions of blocking;
  canonical target locking follows status-deletion lock order to avoid a
  wrapper-versus-source deadlock.
- Status relationship projections now follow Mastodon's serialized-object ID
  maps, while no-op unreblog projections clear viewer relationships recursively.
  Differential setup explicitly drains Rails' asynchronous removal counters
  instead of hiding that fixture limitation in response comparisons.
- Added guarded regressions for reverse-blocked reblogs, removal after blocking
  an author, generated-ID unreblogs, proper-status favourite/bookmark flags,
  and account status-count maintenance. The full 15-case differential suite,
  26-test schema integration, and aggregate `mise run check` gate pass.
- Integrated Mastodon-compatible favourite action policy below the REST read
  boundary. Blocked authors now produce a not-found write result without a
  favourite, notification, delivery, or counter mutation; pure policy and
  restored-schema coverage prove the distinction between readable public
  statuses and actionable favourites.
- Extended action authorization to reblogs and reply targets. Reblogs now
  reject blocked authors and explicit/private audiences, while replies reuse
  the typed status-access policy before any status or counter mutation.
- Hardened durable notification creation against suspension races. Ordinary
  social notifications from suspended senders are dropped when a queued job is
  resolved, while administrative and lifecycle notifications retain their
  intended recipient semantics. Restored schema and worker coverage now pass
  26/26 and 21/21 respectively.
- Hardened known-remote ActivityPub Announce/Undo handling. Known remote
  targets now support nested boosts, unfollow relevance fencing, self-private
  remote boosts, Group notification suppression, and idempotent Undo. Remote
  Note deletion now scopes the target to its author, locks dependent boosts
  before the original status, removes their counters and notifications, and
  preserves tombstone fencing. Restored-fixture worker coverage passes 21/21;
  the library suite passes 147/147.
- Completed the browser-session OAuth ownership contract. New
  `session_activations` now create a `read write follow` token linked to the
  Mastodon `superapp` when available, safely falling back to a nullable
  application, and logout removes both the activation and its token. The
  guarded browser differential case verifies token ownership and cleanup.

## 2026-08-25

- Added Rails-compatible status-edit notification fan-out for local and remote
  notes. Edits now transactionally enqueue replacement-aware `update` jobs for
  local rebloggers and `quoted_update` jobs for accepted local quotes; the Core
  worker dispatches both activity types through the existing policy/resolution
  kernel.
- Added restored-fixture coverage for local and remote edit outbox production,
  durable notification delivery, duplicate handling, and cleanup. The schema
  integration passes 24/24 and the worker integration passes 20/20.
- Completed the basic moderation/reconciliation write path against the pinned
  Mastodon 4.6.5 fixture: report resolution, moderator status deletion,
  account suspension/unsuspension, domain block/unblock, account-stat repair,
  audit records, and durable side effects are transactionally covered.
- Added least-privilege writer validation for moderation tables and the
  operational outbox. Suspension now creates a local `moderation_warning`
  notification job, while direct statuses and non-public replies follow
  Mastodon's counter-cache rules during creation, deletion, and reconciliation.
- Added federated report forwarding as durable ActivityPub `Flag` jobs from the
  instance actor. Forwarding covers the target origin and distinct remote
  reply-server inboxes, matching Mastodon 4.6.5's inbox exclusions and nullable
  `forwarded` persistence semantics.
- Closed an account-status filtering gap: boosts whose source status is missing
  or soft-deleted are now omitted before pagination, matching Mastodon's
  `kept` scope. Restored-fixture coverage verifies the boost disappears when
  its source is deleted.
- Account-stat reconciliation now also repairs `status_stats.replies_count`
  for the target account's statuses from live reply rows, with restored-fixture
  corruption-and-repair coverage.
- Notification-request acceptance now queues a deduplicated durable Core job to
  clear filtered notifications from the accepted sender. Worker integration
  verifies the asynchronous effect, preserving Mastodon's request-acceptance
  timing without requiring Redis. Accepted filtered direct mentions now also
  rebuild sorted, unread `account_conversations` rows transactionally; repeated
  statuses in one conversation are merged idempotently. The least-privilege
  writer contract now includes `account_conversations_id_seq`, while Redis
  streaming merge publication remains deferred.
- Report creation now queues durable Mail-lane messages for eligible staff with
  the flat Mastodon `notification_emails.report` setting. The SMTP worker
  renders the report context and ID without advertising Rustodon's absent admin
  UI route; outbox and SMTP delivery are covered without exposing credentials.
  Report writes also enforce Mastodon's 400-per-UTC-day account limit with a
  PostgreSQL advisory lock, and report errors expose the rate-limit headers.
- Report validation now rejects attached statuses when the target blocks the
  reporter, requires rules for `violation`, and persists omitted `forward` as
  `false`. Writer preflight covers report collections/rules/outbox reads and
  report sequences, while rejecting dangerous explicit table grants.
- Restored-fixture validation now passes the 26-test schema suite, 20-test
  worker suite, 15 differential cases, 3 startup cases, preflight integration,
  and the aggregate `mise run check` gate. Full purge/severance, media privacy,
  timeline/streaming cleanup, and the admin report UI remain deliberately
  deferred.

## 2026-08-11

- Created the Rustodon repository structure and repository-local issue tracker.
- Defined Rustodon as a mostly-in-place replacement for small Mastodon
  installations, with persisted-data and protocol compatibility taking
  priority over Rails implementation parity.
- Selected Mastodon 4.6.5 as the first compatibility target. The initially
  inspected Mastodon `main` checkout identifies itself as 4.7.0-beta.1 and
  contains further schema and behavior changes that should be a separate
  compatibility milestone. Mastodon 4.6.5 already includes quote, collection,
  and keypair data, which v1 must read and preserve even though their mutation
  workflows are deferred.
- Scoped v1 around existing users, PostgreSQL, local Paperclip storage, core
  REST APIs, core ActivityPub federation, and durable PostgreSQL-backed work.
- Explicitly deferred object storage, Elasticsearch, open registration, SSO,
  polls, scheduled posts, quotes, relays, and full administration.
- Chose differential Rails-versus-Rust testing as the primary compatibility
  technique before porting individual Mastodon tests.
- Corrected the initial scope after independent review against the pinned
  Mastodon 4.6.5 tag and tightened physical-schema, signing-key, exclusive-list,
  and read-only acceptance criteria.
- Bootstrapped a single-crate Rust 2024 project with `web`, `worker`, and
  `admin` command surfaces, while deliberately leaving runtime behavior
  unimplemented.
- Pinned Rust 1.97.1, Clang 22.1.8, and cargo-deny 0.20.2 through Mise and added
  shared format, lint, test, dependency-policy, and aggregate check tasks.
- Added CI with immutable action revisions and prohibited unsafe Rust.
- Pinned the Mastodon v4.6.5 compatibility baseline to commit
  `1440d55b139e39ec722c2a3db7f60b66cd889048`, schema version
  `20260611150940`, and the official OCI index digest
  `sha256:77f11d1a6c674664217372d94ccdb9203524c60447827fe74ab6e11466825815`.
- Added a release-versioned, deterministic full database fixture with all 588
  migrations, a full PostgreSQL catalog fingerprint, Paperclip-shaped local
  media, explicit OAuth/relationship/filter/list/quote/collection/poll/keypair
  records, and readable activities for all 17 Mastodon 4.6.5 notification
  types.
- Kept Mastodon 4.6.5's signing-key contract: local account key material uses
  Mastodon's published test RSA key in `accounts`, while remote accounts and
  the remote `keypairs` record contain public material only.
- Identified four narrow fixture-generation normalizations: Mastodon's random
  `timestamp_id()` salt, Rails schema-load timestamps in `ar_internal_metadata`,
  PostgreSQL 14.23's random dump restrict token, and terminal dump formatting.
  The fourth removes only trailing empty `pg_dump` lines and enforces exactly
  one terminal LF while preserving all internal blank lines.
- Added environment-isolated, digest-pinned Podman tooling for source obtain,
  generation, static verification, clean restore/Rails verification, and
  byte-for-byte regeneration. Rails boot did not require a Redis container for
  the verified paths.
- Tightened the fixture after independent review: every Snowflake-backed row
  now encodes its deterministic `created_at` epoch and uses invocation-count
  sequence state; grouped notifications use Mastodon's exact target/hour keys;
  and the moderator role carries `manage_reports` and `manage_users` while each
  notification serializer receives its own recipient user.
- Replaced raw media copies with the pinned Mastodon image's v4.6.5
  Paperclip/libvips processors. The checked tree now contains a 400x400 avatar
  and distinct 600x400 original/588x392 small media styles with coherent
  metadata, blurhash, processing state, and verified output hashes.
- Forced all container runs to pinned `linux/amd64` child manifests, made source
  verification reject dirty trees, derived migration/media inputs from commit
  blobs, and replaced anonymous PostgreSQL storage with labeled volumes that
  are removed and checked after each task.
- Added a small read-only `rustodon::mastodon` compatibility library using
  dynamic SQLx PostgreSQL queries, Tokio, Chrono, JSON, prefix-preserving
  `inet`, and Saphyr YAML parsing. The private pool sets UTC and read-only
  session defaults; no write or Active Record callback surface is exposed.
- Represented IDs as signed `i64`, secrets as opaque redacted values, and
  visibility, notification, polymorphic, integer, and permission-bit values as
  lossless open/raw wrappers. Normal status reads exclude soft-deleted rows;
  notification reads hide filtered rows by default while retaining parent
  metadata when a target status has been deleted.
- Expanded the deterministic 4.6.5 seed with the `-99` instance actor, raw user
  JSON and array edge states, account JSONB, status edits, tag/conversation
  joins, missing v1 relationships and policies, Rails YAML tags, a tombstone,
  an unknown deleted status/filtered notification, and a valid deterministic
  Active Record encrypted keypair envelope. Rails still verifies all 17 known
  notification types separately.
- Added an ignored Podman-backed Rust schema integration task. Its random-port
  LOGIN role receives only `CONNECT`, `USAGE`, and `SELECT`; tests prove INSERT,
  UPDATE, DELETE, TRUNCATE, and schema creation remain forbidden even after the
  session read-only default is disabled.
- Aligned Saphyr 0.0.6 with SQLx's `hashlink` dependency line and pinned the
  compatible `indexmap` lock entry. Cargo-deny exceptions are limited to exact
  Redox, Syn, and Windows transitive versions selected by SQLx and the existing
  CLI stack.
- Closed schema-review gaps by preserving nullable account and notification
  columns, redacting OTP recovery codes, retaining arbitrary-precision JSON,
  and distinguishing unavailable local users from service and login accounts.
- Expanded account, OAuth, status, media, relation, and notification-activity
  mappings needed by the first REST and federation milestones. Direct status
  reads suppress soft-deleted rows while live parent records such as
  notifications and quotes remain lossless when a referenced status is gone.
- Added a test-only Rails-versus-Rust differential harness. It sends one typed
  request to distinct loopback targets, compares exact statuses, declared
  headers, canonical JSON, logical PostgreSQL snapshots, and media hashes, and
  reports focused JSON paths, table keys, and file paths.
- Kept the compatibility boundary on observable behavior rather than Rails
  internals: ActivityPub documents, durable job intent, and media artifacts have
  typed comparison slots, while callback counts, query ordering, Redis keys,
  and Sidekiq representation are deliberately excluded.
- Added exact, format-validating normalization rules for request IDs, generated
  timestamps, and prefixed random test tokens. Broad key deletion, wildcard
  paths, array reordering, and number coercion are not allowed.
- Added guarded differential orchestration using independently marked clones of
  the pinned database and media tree, a pinned empty Redis, and pinned Mastodon
  Puma. Rust receives SELECT-only clone credentials; loopback URLs, database
  comments, media markers, canonical paths, and symlink absence are validated
  before requests run.
- Added typed loading for the v1 Mastodon environment surface: canonical
  domains, PostgreSQL precedence, local Paperclip paths, trusted proxies, SMTP,
  cryptographic secrets, and optional Sidekiq Redis inspection. Secret wrappers
  zeroize on drop and redact `Debug`, `Display`, and validation failures.
- Added Rails 8.1 Active Record AES-256-GCM key decryption with current
  PBKDF2-SHA-256 and legacy SHA-1 read fallback, plus semantic RSA key matching
  and an in-memory RSA-SHA256 sign/verify check.
- Added `rustodon preflight` with stable fatal/warning codes. It compares all
  expected migration versions and the v1-critical physical PostgreSQL catalog,
  validates `timestamp_id()` and its seven sequences without executing them,
  checks canonical identifiers and operational local signing keys, and rejects
  active workflows outside v1.
- Kept preflight on the cutover contract rather than Rails implementation
  details. It rejects active object storage and SSO instead of parsing every
  provider option, ignores unrelated extension tables, and checks logical
  Sidekiq work rather than Redis's internal key layout beyond read-only queue
  discovery.
- Added read-only authentication for existing Mastodon OAuth bearer tokens with
  exact Doorkeeper revocation and expiration boundaries, endpoint-specific
  broad/granular scope alternatives, application-only principals, and Mastodon
  user/account functional-state ordering. The joined lookup never selects the
  bearer or refresh token, application secret, password, OTP material, or
  recovery codes.
- Deliberately omitted Mastodon's once-per-day access-token and user sign-in
  metadata writes. They remain deferred to the authenticated-write phase; the
  OAuth fixture and differential tests prove Rust authentication leaves every
  row unchanged. Differential setup refreshes that metadata only in its
  transient template before cloning so Rails does not introduce an expected
  tracking write during read-only response comparison.
- Added database-free REST serializers backed by batched read-only projections
  for Mastodon 4.6.5 accounts, credentials, relationships, statuses, media,
  polls, quotes, collections, filters, markers, notifications, and instance
  v1/v2 responses. IDs, dates, nullable fields, rendered HTML, authenticated
  state, and all 17 known notification types are differentially verified.
- Expanded compatibility fixtures for cached Paperclip media, profile mentions,
  historical null-local statuses, legacy null-type notifications, notification
  pagination/grouping stress, and authorization-sensitive quote states. Quote
  expansion is explicitly bounded and self-quote coverage proves cyclic data
  cannot recurse indefinitely.
- Promoted the first REST read surface to the production Axum web process:
  instance v1/v2 and rules, disabled translation languages, account show,
  lookup, verify-credentials, relationships, statuses, followers/following,
  status show, and context now read directly from PostgreSQL.
- Kept root `StatusPolicy` authorization, account-status selection, and context
  member filtering as distinct selectors. Current follows, active and silent
  mentions, author blocks and domain blocks, viewer blocks/mutes/domain blocks,
  suspended or silenced authors, and soft deletion are covered independently.
- Added stable Mastodon-compatible pagination for account statuses and follow
  collections, including `max_id`, `min_id`, `since_id`, endpoint limits, exact
  `Link` ordering, pin-time ordering, self-replies, and edited-out media.
- Expanded the deterministic fixture with pending/unconfirmed local accounts, a
  functional unrelated OAuth viewer, multiple follows and pins, blocked and
  domain-blocked thread members, and a silenced viewer's own reply. Dedicated
  SQL and HTTP authorization matrices prove private, direct, and limited status
  denial across show, account-status, and context endpoints.
- Hardened quote projection boundaries after independent review: unauthorized
  targets, including targets hidden by an author-side domain block, no longer
  enter nested status or target-link projections before serialization.
- Replaced Redis-derived home and list reads with direct PostgreSQL selectors
  and added public, hashtag, list, favourites, bookmarks, blocks, and mutes
  routes with endpoint-specific OAuth scopes and cursor contracts.
- Matched Mastodon timeline filtering for feed-access settings, follows and
  chosen languages, replies, boosts, exclusive lists, blocks, mutes, domain
  blocks, custom filters, hashtag normalization, and edited-out media.
- Expanded deterministic Rails feed materialization for followed tags and
  owner self-membership, then proved the timeline fixture byte-for-byte
  reproducible and all read responses differentially compatible with 4.6.5.
- Centralized shared REST protocol behavior around an explicit 21-route
  inventory, including CORS/preflight, trailing slashes, cache and `Vary`
  headers, Rails-compatible errors, request-size enforcement, and pagination
  contracts without advertising unsupported routes.
- Added database-referenced local Paperclip serving for accounts, media files
  and thumbnails, custom emoji, preview cards and provider icons, and site
  uploads, including existing processed audio/video paths. REST serializers and
  request authorization share cache-prefix, ID-partition, style, filename, and
  URL-escaping rules.
- Added Mastodon-compatible `GET`, `HEAD`, conditional, single-range, and
  streaming multipart-range responses with immutable cache, CSP, MIME,
  Last-Modified, and Rails-visible error behavior verified against pinned
  Mastodon 4.6.5.
- Hardened filesystem reads with a startup-retained `openat2` root descriptor,
  no symlink traversal, clean decoded components, regular-file checks, and
  best-effort no-atime reads. Differential snapshots now compare media mode,
  owner, group, and modification time in addition to bytes and hashes.
- Added bounded Rack-compatible query/form parsing and registered JSON body
  parsing with scalar, null, array, hash, collision, depth, count, byte-limit,
  numeric coercion, and body/query merge semantics verified directly against
  Mastodon 4.6.5.
- Closed the REST protocol milestone after independent review and sequential
  `check`, fixture restore/reproducibility, schema, preflight, and complete
  five-case differential gates all passed.
- Added an explicit, transactional `rustodon admin migrate-operational-schema`
  command for the separately owned `rustodon` namespace. Version 1 stores
  durable jobs, outbox events, idempotency keys, ordering markers, domain
  health, and worker/scheduler heartbeats without foreign keys or changes to
  Mastodon's `public` schema.
- Serialized operational DDL with both Rustodon and Active Record 8.1 migration
  locks, validated all 71 relations read by current Rustodon code plus the
  Snowflake sequences and `timestamp_id()` before and after DDL, and rejected
  event triggers, all-table publications, behavior hooks, unsafe collations,
  unpopulated materialized views, and unsupported Mastodon schema versions.
- Pinned an OID-independent operational catalog fingerprint that records the
  original schema owner, complete ACLs, schema-qualified collations, comments,
  dependencies, extension membership, triggers, rules, policies, and other
  PostgreSQL 14 namespace object classes. Fresh, repeat, concurrent absent and
  empty-schema creation, owner reassignment, grants, and attached-object drift
  are covered by the isolated integration gate.
- Closed the operational-schema milestone after independent review and
  sequential `check`, operational/preflight integration, fixture
  restore/reproducibility, schema integration, and all six differential cases
  passed. Before/after catalog, schema, data, owner, and ACL snapshots plus
  pinned Rails verification prove Mastodon rollback remains possible.
- Added PostgreSQL durable jobs and transactional outbox dispatch with delayed
  execution, fenced renewable leases, final-attempt crash recovery, logical-key
  deduplication/cancellation, bounded deterministic jitter, dead letters, and
  worker/scheduler heartbeats. Dispatch and cancellation serialize on keyed
  outbox rows so a committed cancellation cannot leave runnable work behind.
- Added a handler registry with explicit lane capability and independent remote
  HTTP/media semaphores. Leases renew while waiting for permits; stale handlers
  cannot acknowledge expired or replaced leases. Infrastructure currently
  registers only maintenance cleanup, making `maintenance` the truthful default
  and rejecting configured lanes without handlers.
- Required a distinct `NOINHERIT` runtime database login with exact operational
  DML/sequence grants and read-only Mastodon access. Startup rejects schema
  owners, memberships, database/schema creation, direct or `PUBLIC` Mastodon
  writes, operational ACL drift, and unsupported schemas.
- Added readiness and bounded dead-letter administration, poll-cadence outbox
  draining independent of heartbeat cadence, and scheduler/handler shutdown
  separation. Shutdown joins the sole heartbeat writer, withdraws readiness,
  drains handlers within one absolute deadline, and never acknowledges aborted
  work.
- Closed the durable-worker milestone after independent review and sequential
  `check`, worker/operational/preflight integration, fixture restore and
  reproducibility, least-privilege schema integration, and all six differential
  cases passed. Eight PostgreSQL integration cases cover queue concurrency,
  final-attempt crashes, duplicate effects, permit-wait renewal, dispatch versus
  cancellation, retries/dead letters, runtime privileges, readiness, and
  shutdown.
- Added shared production startup validation before web bind or worker claims.
  The bounded, read-only checks cover configuration, media, the pinned Mastodon
  schema, signing keys, canonical domains, active workflows, the operational
  schema, the direct runtime role, and its required Mastodon and operational
  privileges. Operational migrations remain explicit and perform no runtime DDL.
- Added strict listener parsing, graceful serving, dependency-free `/health`,
  and bounded `/ready` checks for database availability plus `SELECT` on every
  v1-critical Mastodon relation. Worker startup publishes initial worker and
  scheduler heartbeats before claim loops begin.
- Added explicit trusted-proxy handling: forwarding metadata is ignored unless
  `TRUSTED_PROXY_IP` trusts the peer, malformed trusted forwarding fails closed,
  and effective authorities are constrained to configured canonical and media
  hosts. Absolute Paperclip media uses the sanitized effective authority.
- Closed production startup safety after independent review and sequential
  `check`, startup, worker, operational/preflight integration, fixture restore
  and reproducibility, least-privilege schema integration, and all six
  differential cases passed. Real-process tests prove fatal startup has no web
  bind or worker side effects and readiness degrades after database or required
  relation privilege loss without affecting liveness.
- Began the cross-surface policy foundation with typed, pure status audience and
  context decisions shared by root reads, context filtering, and quote targets.
  Unknown visibility, deletion, and suspension fail closed before owner
  exemptions; private/direct audiences preserve Mastodon 4.6.5 semantics.
- Made raw status graph loading private so public root reads must authorize
  first. Added an isolated database mutation regression proving an undeleted
  unknown visibility cannot appear as a root, context member, quote target, or
  shallow target ID. Seven schema integration cases and the focused Mastodon
  differential authorization matrix passed, and independent review found no
  blocker in this slice.
- Added exact Mastodon 4.6.5 local-role semantics for all 23 permission bits,
  including EVERYONE inheritance, direct administrator expansion, any-of
  permission checks, strict position hierarchy, and highlighted moderation block
  bypass. Raw and effective masks are distinct types so action policy cannot
  accidentally skip inheritance.
- Keyed credential and restricted-feed permission loading to the exact OAuth
  resource-owner user/account pair. Crossed identities fail closed, and exact
  owner settings and role now drive both flattened account fields and top-level
  credentials. Startup and preflight reject a missing mandatory EVERYONE role.
  Eight schema integration cases, preflight clone rejection, Clippy, and an
  independent role-policy review passed.
- Added typed account lifecycle decisions for limited, moved, memorial,
  temporary suspension, and permanent unavailability. Browser authentication
  is intentionally distinct from functional API access; OAuth reads deletion
  requests and keeps the instance-actor suspension exemption.
- Added reusable global-domain decisions with Mastodon-compatible transitional
  IDNA normalization, exact label boundaries, longest-parent precedence,
  silence/suspend/noop and media/report controls, and fail-closed unknown or
  NULL severity. Independent comparison against the pinned Mastodon lifecycle
  and domain models found no blocker; federation call sites remain owned by the
  later signature, fetch, inbox, and delivery slices.
- Began the public federation discovery slice with WebFinger, host-meta, NodeInfo
  2.0, local actors, public Notes, outbox, followers, and following collections.
  Local ActivityPub URLs derive from the account ID scheme, local endpoint fields
  are derived when legacy rows are blank, accepted quote targets flow through the
  existing HTML formatter, and browser requests preserve Mastodon's absolute
  HTML redirects instead of returning JSON-LD.
 - Added a guarded `federation_discovery` Rails-versus-Rust differential case for
   malformed and unknown WebFinger resources, discovery documents, actor and
   Note fields, collection totals, pagination identifiers, and HTML redirects.
   The case passes against the pinned Mastodon 4.6.5 fixture. The fixture's
   `/actor` request currently returns HTTP 500 despite the pinned upstream request
   spec requiring 200, so that inconsistent instance-actor request remains a
   separate follow-up rather than weakening the supported discovery gate.
- Hardened the discovery slice after source review: unavailable actors mask
  profile fields, WebFinger authorities preserve explicit ports, local quote
  URIs respect numeric account IDs, suspended collection members remain
  representable, collection page presence matches Rails, ActivityPub Accept
  negotiation honors quality values, and outbox data errors fail closed.
- Restored Rails prefix coercion for REST route IDs while keeping ActivityPub
  account IDs constrained, and added the encoded-ID regression to the guarded
  federation case. Formatting, Clippy, 42 unit tests, all 7 differential cases,
  and startup integration pass.
 - Moved `GET /api/v1/collections/:id` from a differential-only fixture route
   into production routing, reusing the visibility-aware collection projection.
   Added exact status-source serialization and status-history snapshots, including
   historical media ordering/descriptions, polls, legacy quote states, and strict
   token handling. The core REST differential now exercises all three production
   endpoints and remains compatible with Mastodon 4.6.5.
 - Added reverse status actor reads for favourites and boosts. Their selectors
   preserve Rails association/status cursor IDs, public/unlisted root policy,
   suspended-account exclusion, viewer block/mute filtering, application-only
   token behavior, and ordered pagination links.
 - Added production account search with required user authentication, exact
   stored local/remote handle matches, PostgreSQL full-text ranking, following
   filtering, and Rails-compatible limit/offset behavior. `resolve=true` is
   explicit for complete remote handles, keeping network resolution isolated
   behind the later safe remote-fetch milestone.
 - Moved `GET /api/v1/markers` into production routing. It preserves
   user-scoped marker ownership, scalar/array/unknown timeline semantics,
   private cache/Vary behavior, and the existing Rails differential/auth matrix;
   marker writes remain deferred.
 - Moved `GET /api/v2/filters` into production routing, reusing the existing
   account-scoped filter, keyword, and status projections. The endpoint now
   participates in the production protocol path; filter writes remain deferred.
 - Added production `GET /api/v1/lists` using the existing account-owned list
   query. The four-field response, replies-policy mapping, trailing slash,
   private headers, and auth/owner-isolation cases are differentially covered;
   list writes remain deferred.
 - Added production `GET /api/v1/featured_tags` with account-owned tag joins,
   tag-name fallback, account-tag URLs, string counts/date serialization, and
   broad/granular/owner-isolation differential coverage.
 - Added public production `GET /api/v1/accounts/:id/featured_tags` with
  unavailable/suspended-account handling, public access independent of token
   scopes, account-tag URLs, and anonymous/trailing/missing-target coverage.
 - Added public production `GET /api/v1/custom_emojis` using the Rails `listed`
   scope: local, enabled, picker-visible emojis only, with category/featured
   metadata and Paperclip URLs. Anonymous and authenticated trailing-slash
   responses are covered by the Rails differential suite.
 - Added production `GET /api/v1/featured_tags/suggestions` with required
   account-read authentication, Rails recent-status ranking, featured-tag
   exclusion, relationship booleans, trailing-slash support, and differential
   coverage. The old fixture-only authentication route was removed.

## 2026-08-21

- Moved `GET /api/v1/notifications` and `GET /api/v2/notifications` from
  differential-only fixture handlers into production routing. Shared read
  projections now cover Rails type/exclusion filters, filtered rows, v1/v2
  cursor pagination, grouped-type selection, partial avatars,
  repeated array parameters in `Link` headers, and trailing slashes.
- Added differential coverage for notification pagination, grouping, exclusions,
  fallback serialization, and both production route versions. The focused core
  REST case passes against Mastodon 4.6.5; notification writes, clear, and
  dismiss remain deferred.
- Added production v1/v2 notification unread-count and show reads. Counts honor
  notification markers, type exclusions, limits, and v2 grouped types; show
  reads enforce account ownership and preserve v1/v2 serializer shapes. Clear
  and dismiss remain blocked on the writable repository foundation.
- Closed the remaining notification read parameter-parity gaps for blank scalar
  grouped types and scalar supported types, then passed the full local check,
  all seven guarded read differential cases, and startup safety integration.
- Began the writable transaction foundation with a separate typed
  `WriteRepository`. Marker updates require a functional `write:statuses`
  bearer, use Rails-compatible `INSERT ... RETURNING` and optimistic locking,
  and are covered by owner-role schema integration. Atomic idempotency claims,
  JSON result recording, optional outbox composition, a least-privilege writer
  role, and a guarded Rails-versus-Rust marker differential case now pass; web
  startup now accepts an explicit optional `WRITE_DATABASE_URL` while retaining
  the read-only default, and user-facing writes remain deferred.
- Added v1/v2 notification clear and dismiss POST routes. They require
  `write:notifications`, delete only the authenticated account's rows, handle
  grouped keys, and reconcile filtered notification requests transactionally.
   The guarded differential suite now covers these writes alongside the
   marker foundation.
- Added an internal idempotent notification-creation kernel for mention, status,
  follow-request, and poll activities. It validates recipient ownership, drops
  unavailable/self/blocked/muted/domain-blocked/conversation-muted activities,
  applies the supported notification-policy actions, rejects silent or deleted
  source activities, and updates filtered mention requests with capped counts.
  Owner-role schema integration now covers retry idempotency and recipient
  validation; policy unit tests cover accept/filter/drop precedence.
- Tightened the differential writer role with read-only access to the source and
   policy tables needed by notification creation while retaining write access
   only for markers, notifications, and notification requests. The full local
   check, startup integration, schema and operational integrations, and all nine
   guarded differential cases pass after these changes.
- Expanded the notification creation kernel to all 17 stored Mastodon 4.6.5
  activity/type associations. Groupable favourite, reblog, follow, and admin
  sign-up notifications now use Rails-compatible target/hour keys; update,
  quoted-update, and collection-update retries replace prior rows. Added
  recipient validation, staff mention bypass behavior, filtered quote request
  updates, ungrouped v2 dismissal, exact clear/delete-all behavior, and
  full owner-role integration coverage.
- Added `POST /api/v1/markers` with Rails nested parameters and trailing-slash
  routing. Multi-timeline updates run in one writer transaction, return exact
  marker serializers, and participate in the guarded differential suite.
 - Added account-owned conversation index, read, unread, and delete endpoints.
   Conversation serialization now loads sorted participants and visibility-aware
   last statuses, uses account-conversation IDs, preserves Rails deleted-last-
   status validation, and participates in the notification/write differential
   case with database restoration.

## 2026-08-22

- Persisted notification group buckets in Rustodon-owned expiring ordering
  markers so dismissing the final row preserves Rails' 12-hour grouping window.
  The owner-role schema regression and least-privilege notification differential
  case pass without changing Mastodon's public schema.
- Added production `GET /api/v1/followed_tags` with account-owned cursor
  pagination, Rails-compatible legacy `follow` scope support, tag relationships,
  empty seven-day history, trailing-slash routing, and differential coverage.
- Added production `GET /api/v1/follow_requests` with suspended-requester
  filtering, full account serialization, legacy follow-scope authentication,
  cursor pagination, trailing-slash routing, and differential coverage.
- Added production `GET /api/v1/preferences` with exact user/account settings
  ownership, Rails preference defaults and locale fallback, trailing-slash
  routing, and differential coverage.
- Added the reusable legacy Cavage HTTP signature primitive: RSA-SHA256 GET/
  POST signing and verification, exact body digests, required signed headers,
  one-hour clock skew, explicit key-ID binding, `expires` enforcement, and
  Mastodon's queryless request-target fallback. Fixed Mastodon GET/POST vectors,
  malformed cases, digest tampering, and private-key Debug redaction are
  covered; RFC 9421 and `hs2019` remain explicitly unsupported until transport
  integration is designed.
- Re-ran the full local check, 7 HTTP-signature tests, all 9 guarded
  differential cases, 12 schema tests, operational-schema integration, and
  startup safety integration successfully after the signature slice.
 - Added persisted ActivityPub public-key resolution for canonical actor IDs and
   legacy `acct:` aliases, retaining ownership, revocation, and expiry metadata.
   Existing actor GETs now optionally verify legacy signatures before rendering;
   the signed actor request passes the pinned Mastodon 4.6.5 differential case.
   Inbox enforcement, remote fetching, outbound signing, and broader signed-GET
   policy remain intentionally separate transport milestones.

- Added the first production status-interaction writes: bookmark and favourite
  create/remove operations use the optional least-privilege writer, preserve
  original-status targeting, maintain favourite counters, and create supported
  favourite notifications. Rails' POST `unbookmark` and `unfavourite` routes
  are mirrored explicitly; the writer fixture grants only the required public
  tables and sequences.
- Added owner-role schema coverage and folded four interaction requests into the
  guarded write differential case, including database snapshots and restoration.
  The full local check, 14-test schema integration, operational schema and
  startup integrations, and all 9 guarded differential cases pass. Boost writes,
  concurrent interaction proof, and complete interaction/outbox behavior remain
  open under the status-social-interactions issue.
- Extended the interaction writer with Rails-compatible POST reblog/unreblog
  routes. Boost creation now uses a hashed transactional advisory lock, the
  Mastodon timestamp ID function, account visibility defaults, original-status
  reblog counters, account status counts, and soft removal on unboost. Schema
  coverage proves duplicate creation and removal are idempotent; the guarded
  HTTP case covers generated response shape and restoration. Rails' second
  asynchronous unreblog response remains excluded from exact body comparison
  until its worker-dependent serializer behavior has a stable contract.
 - Started account relationship writes with local follow/unfollow routes. The
   writer preserves reblog/notification/language options, chooses direct follows
   versus pending requests, locks relationship pairs, updates both account
   counters, and emits the supported local follow notification. Schema and
   guarded differential coverage pass for duplicate retries and restoration;
   block, mute, follow-request transitions, and remote delivery remain open.
 - Added local block/unblock and mute/unmute writes with Rails scope contracts,
   transactional advisory locking, idempotent retries, follow cleanup on block,
   notification visibility flags, and nullable mute expiration. Expanded the
   deterministic fixture's follow token to cover the new write scopes, regenerated
   the database dump, and passed 16/16 owner-role schema tests plus all 9 guarded
   differential cases. Active mute expiry/worker cleanup, follow-request
   transitions, remote delivery, and complete outbox intent remain open.
 - Added local follow-request authorize/reject and remove-from-followers routes.
   Authorization preserves request options and URI while moving the request to a
   follow, updates both account counters, removes the dependent request
   notification, and emits the supported local follow notification. The 17-test
   owner-role schema suite and all 9 guarded differential cases pass with source
   and target relationship rollback; mute expiry, block cleanup, federation,
   and outbox behavior remain open.
 - Added the first status-write lifecycle slice: text-only `POST /api/v1/statuses`
   supports all five visibility values, content warnings, sensitivity, language,
   application attribution, quote-policy defaults, conversations, status stats,
   account counters, and operational idempotency replay. The 18-test owner-role
   schema suite and all 9 guarded differential cases pass; media, replies,
   mentions/tags, edits, deletes, and distribution remain open.
 - Extended text status creation to authorized replies, inheriting the target
   conversation and language while updating parent reply counters. Differential
   coverage now creates all five visibility variants plus a reply and restores
   generated statuses, conversations, status statistics, and account counters.
- Hardened the status differential cleanup to delete generated per-target rows
  instead of mutating the fixture's referenced status. The guarded notification
  case now remains isolated after status deletion, with the full 9-case suite
  passing again.
- Extended status deletion to discard reblogs, remove status pins, and maintain
  the owner's status counter transactionally. Owner-role schema coverage now
  proves reblog and pin cleanup, while the least-privilege writer grants the
  required `status_pins` table and operational outbox sequence.
- Scoped status idempotency keys by account, bound fingerprints to reply
  targets, and evicted expired keys during claims. Unit and owner-role schema
  tests cover account/reply identity, canonical whitespace, and fresh creation
  after expiry.
- Removed stale follow, follow-request, favourite, and reblog notifications
  during relationship teardown. Differential interaction coverage now snapshots
  affected notification rows and verifies rollback to the Rails baseline.
- Block creation now clears the matching notification-permission exception
  transactionally, with owner-role schema coverage and least-privilege grants
  proving the cleanup.
- Notification policy now treats expired notification-hiding mutes as inactive;
  an isolated owner-role regression covers the expired-mute path.
- Added the text/settings/fields slice of `PATCH /api/v1/accounts/update_credentials`
  with `write:accounts` authorization, transactional account and user-setting
  updates, profile field verification preservation, attribution-domain
  normalization, bot/visibility flags, and Rails-compatible profile hashtag
  refresh. Local profile field links now carry the `rel="me"` contract.
- Added nested form parsing, owner-role schema coverage, least-privilege grants,
  and Rails differential coverage for profile updates, including wrong-scope
  rejection and rollback. The guarded schema suite is now 20/20, the full
  differential suite remains 9/9, and the repository check passes; avatar/header
  media processing and removal remain open.
- Added local avatar/header multipart updates and the Rails-compatible profile
  deletion routes. Paperclip writes are path-safe and non-destructive, filenames
  are obfuscated and MIME-normalized, avatar/header styles enforce decoded image
  limits, GIF originals retain their frames, and static GIF derivatives are
  generated. Owner-role metadata coverage, 11 Paperclip tests, the 52-test local
  suite, guarded schema 20/20, guarded differential 9/9, and `mise run check`
  pass. HTTP media differential rollback and ActivityPub/profile side-effect
  workers remain open.
- Added image-only v1/v2 media CRUD with `write:media` ownership checks,
  descriptions, focus metadata, 8,294,400-pixel original limits, 230,400-pixel
  small derivatives, Paperclip paths, generated blurhash values, and safe
  database/file cleanup. Added media writer grants, route inventory coverage,
  Paperclip tests, and an owner-role create/update/delete test. The guarded
  differential case now covers v1 create/update/delete, blank-focus no-op
  updates, v2 image create, readable original/small artifacts, and rollback;
  all 10 cases pass. Blank/null focus now follows Rails' no-op setter behavior,
  and full-suite fixture orchestration makes media roots writable only when the
  media write case is included. Animated-GIF/video transcoding, HEIC/AVIF
 conversion, exact libvips byte/blurhash parity, and crash-time database/file
 compensation remain open.
- Extended status creation with Rails-compatible ordered owned-media attachment,
  media-only posts, omitted-language fallback to `en`, hashtag persistence, and
  featured-tag counter updates. Existing local/remote account mentions now
  persist mention rows and invoke the notification policy kernel. Owner-role
  schema coverage and the full 10-case differential suite pass with
  status/media/tag/mention rollback; remote account resolution, edits,
  delete-media handling, and post-commit distribution remain open.
- Added the next status lifecycle slice with owner-authenticated `PATCH
  /api/v1/statuses/:id`. Text, content warnings, sensitivity, language, and
  ordered media changes now run transactionally with Rails-compatible initial
  and current `status_edits` snapshots, hashtag/featured-tag refresh, mention
  replacement, and policy-aware mention notifications. Differential coverage
  compares edited HTTP/database state, history row counts, and media removal;
  `mise run check`, owner-role schema 21/21, and all 10 guarded differential
  cases pass. Remote account resolution, delete-media semantics, explicit
  idempotency-key edit replay, and post-commit distribution/removal remain open.
- Added Rails-compatible `PUT /api/v1/statuses/:id` as an alias of the status
  update action. The guarded differential case now proves identical PUT replay
  is a no-op with no extra edit snapshot; owner-role schema coverage proves the
  same repository behavior. The image media CRUD and account profile update
  issues now satisfy their written acceptance criteria and were archived, while
  codec, federation, worker, and crash-hardening follow-ups remain open.
- Audited timed mute cleanup against the durable worker boundary. Worker
  integration confirms the runtime role is intentionally read-only on Mastodon
  tables, so expiry cleanup cannot be added by granting direct deletes; it
  requires a separate configured writer pool and remains open under the
  relationship-write issue.
 - Added the writer-backed timed mute expiry slice. Mute writes now record an
   atomic `rustodon.mastodon.delete_mute` outbox event and remove pending events
   on mute removal or renewal; workers execute expiry through an optional
   separately configured Mastodon writer pool while retaining a read-only
   default runtime role. Owner-role schema, worker integration, and focused
   differential verification pass. Remote relationship transitions, block
   side effects, complete outbox intent, and concurrency proof remain open.
 - Added transactional status `delete_media` handling. Owner-authenticated
   deletion now detaches or removes unreported media, retains media for
   unresolved reported statuses, removes pins, reconciles original/reblog
   counters and owners, and casts JSON numeric `0` as false. Owner-role schema
   coverage, the focused Rails-versus-Rust HTTP case, the full repository check,
   and all 10 guarded differential cases pass for both deletion modes;
   asynchronous distribution/removal remains in the parent lifecycle issue.
 - Completed local block teardown. Block writes now share the recipient
   notification advisory lock, preserve outgoing follow requests, reject
   incoming requests, and atomically clear the block owner's notifications,
   notification requests, and conversations containing the blocked account.
   Self-block is a Rails-compatible no-op; owner-role schema, relationship
   differential coverage, the full repository check, and all 10 guarded cases
   pass. Remote delivery and concurrent relationship proof remain open.
 - Added concurrent follow, block, and mute requests to the relationship
  differential case. The proof compares stable final relationship options,
  counters, synchronous notification cleanup, and Rails' unique-validation
  loser responses, restoring both fixture databases after each case. Remote
  relationship transitions and asynchronous outbox delivery remain open.
  - Added concurrent bookmark, favourite, and reblog create/remove coverage to
    the status interaction differential case. Rails' exact duplicate-record
    loser response is required, generated reblog IDs are normalized only after
    shape validation, and final assertions reject duplicate rows while comparing
    status/account counters. Both fixture databases restore interaction,
    conversation, notification, and notification-request state; queued Rails
    favourite/reblog removal effects are drained explicitly because the guarded
   fixture has no Sidekiq consumer. Owner-role schema 21/21, repository check,
   and all guarded differential cases passed at that point.

## 2026-08-23

- Added production `GET /api/v1/lists/:id`, `GET
  /api/v1/lists/:id/accounts`, and `GET /api/v1/accounts/:id/lists` reads.
  List ownership, `read:lists` scope alternatives, suspended-member filtering,
  cursor pagination, Rails' `limit=0` ordering, account-list membership, and
  trailing-slash routes now match the pinned fixture. Added guarded differential
  coverage for ownership, scope failures, missing records, cursors, and
  unlimited results.
- Added direct media-show differential coverage so the production
  `GET /api/v1/media/:id` response is checked after create/update and before
  deletion. The focused core and media differential cases pass after the slice.
- Added production account-owned and featured-in collection reads for the
  preserved collection data, including Rails authentication differences,
  offset pagination, discoverability/suspension rules, collection envelopes,
  and trailing-slash routes. Core differential coverage now exercises anonymous,
  authenticated, missing-record, and wrong-scope cases.
- Added production notification-request list/show reads with Rails-compatible
  account ownership, functional-user authentication, request/status graphs,
  max/min/since cursor behavior, and bounded pagination links. Added the
  persisted `updated_at` projection needed by the REST serializer; core
  differential coverage now includes list, show, cursor, scope, and missing
  request cases.
- Added production notification-request accept/dismiss member and bulk POST
  routes. Immediate Rails effects now match: owner-scoped member 404s, ignored
  missing bulk IDs, permission insertion on acceptance, request deletion, and
  `{}` responses. Recipient advisory locks serialize decisions with filtered
  notification creation. Differential coverage compares auth failures,
  scalar/array IDs, permission/request state, and unchanged notifications under
  the fixture's no-Sidekiq boundary; asynchronous unfilter/cleanup workers
  remain a separate lifecycle follow-up.
- Added production `GET /api/v1/statuses/:id/quotes`. Accepted quote sources
  now use root-status authorization, visibility-aware status projection graphs,
  Rails' status/quote ordering and quote-ID cursors, required
  `read:statuses` authentication, private protocol headers, and trailing-slash
  routing. The core Rails differential covers success, cursors, missing auth,
  and wrong scopes.
- Added authenticated `GET /api/v1/notifications/requests/merged` in both
  slash forms. It reports the synchronous settled state while the
  Redis-backed notification unfilter worker remains deferred; differential
  coverage now includes success, trailing slash, missing auth, and wrong scope.
- Added authenticated v1/v2 notification-policy reads. The serializers preserve
  Rails defaults, v1 boolean compatibility, v2 accept/filter/drop strings,
  unknown persisted values, pending-request summaries, suspended-sender
  filtering, and trailing-slash behavior. Differential and owner-role schema
  coverage now prove the policy projection.
- Added transactional v1 boolean and v2 enum notification-policy updates with
  account advisory locking and default-preserving upserts. The notification
  write differential now compares auth failures, response bodies, policy state,
  and rollback restoration for both API versions.
- Added status conversation mute/unmute writes with `write:mutes` authorization,
  idempotent persistence, status response serialization, trailing-slash routes,
  and guarded interaction rollback coverage.
- Added owner-only status pin/unpin writes with `write:accounts` authorization,
  Rails validation boundaries, idempotent pin persistence, trailing-slash routes,
  and guarded rollback coverage.
- Added public `POST /api/v1/apps` registration with Doorkeeper-compatible
  credential persistence, secure generated uid/secret values, default scopes,
  redirect URI normalization, VAPID metadata, trailing-slash routing, and
  differential rollback coverage that preserves existing OAuth tokens.
- Added authenticated `GET /api/v1/apps/verify_credentials` in both slash
  forms, with public-only application serialization and differential coverage
  for application-backed, application-only, scope-independent, revoked,
  expired, unknown, and missing bearer tokens. Application credentials now use
  Doorkeeper's unpadded URL-safe Base64 shape over 32 random bytes.
- Tightened app registration to Mastodon's 60/2,000/2,000 field limits,
  configured scope set, OOB redirect exception, URL scheme/host checks, and
  fragment/relative/forbidden-scheme rejection. Distributed registration
  throttling, optional VAPID nulls, and parameter-based bearer transports remain
  deferred with the rest of the OAuth server.
- Matched Mastodon's app-scope edge behavior: array-form scopes fall back to
  `read`, and scalar scopes are deduplicated in original order before storage.
- Added the browser-independent `POST /oauth/token` client-credentials grant.
  Client-secret POST and Basic authentication, default and application-bounded
  scopes, active-token reuse, new application-only token persistence, exact
  Doorkeeper response headers, and cleanup through application deletion now pass
  guarded Rails differential coverage.
- Added `POST /oauth/revoke` for client-authenticated access-token revocation,
  ownership enforcement, token-type hints, unknown-token idempotence, and
  persisted `revoked_at` state.
- Added `GET`/`POST /oauth/userinfo` with profile-scope authorization and
  Mastodon-compatible OIDC claims, cache policy, and `Vary` behavior.
- Added `GET /.well-known/oauth-authorization-server` with the pinned
  authorization, token, userinfo, revocation, registration, scope, grant, and
  PKCE metadata. The guarded OAuth differential case compares the exact JSON
  and cache headers.
- Added Doorkeeper-compatible `access_token` and `bearer_token` parameter
  authentication to the API middleware for both query and form parameters.
  Explicit `Authorization` headers remain authoritative, and the focused
  bearer-authentication differential case passes against Mastodon 4.6.5.
- Added browser authentication foundations: bcrypt password verification,
  SHA-1 TOTP with replay protection, backup-code matching and consumption,
  durable login activity limits, transactional `session_activations`, secure
  session/CSRF cookies, and `/auth/sign_in`, `/auth/session`, and sign-out
  routes. Guarded browser-auth differential vectors pass; WebAuthn, HTML form
  rendering, recovery mail, and distributed limits remain open.
- Added the OAuth authorization-code path: browser-session consent at
  `/oauth/authorize`, one-time persisted grants, S256 PKCE verification,
  redirect/state handling, denial responses, and authorization-code token
  exchange. Guarded differential coverage proves token issuance, grant
  revocation, replay rejection, and response compatibility.
- Expanded the authorization-code differential case through `/oauth/userinfo`
  using the generated profile-scoped token. Added fixture cleanup privileges
  for authentication and OAuth rows so the full 12-case differential suite
  runs concurrently without leaking state; all 12 cases pass.
- Added transactional password recovery foundations: bcrypt administrator reset
  through `rustodon admin reset-password`, single-use SHA-256 reset tokens with
  six-hour expiry, CSRF-protected reset request/edit/update pages, session and
  OAuth-token invalidation, and guarded request/update/replay/expiry coverage.
  The full guarded differential suite now passes 13/13 cases. SMTP confirmation
  delivery and CLI user creation remain open.

## 2026-08-24

- Added durable local status-notification production. Status creation now records
  a transactional `rustodon.mastodon.notify_status` Core-lane outbox event;
  configured workers select active local followers with `notify=true`, restrict
  limited/direct delivery to mentioned followers, and reuse the idempotent
  notification-policy kernel. Deletion cancels pending events and retries do not
  duplicate rows. Owner-role schema coverage now proves active-follower delivery
  and the guarded notification write case passes; feed fan-out, edit/quote
  updates, streaming, and remote delivery remain separate milestones.
- Made optional VAPID configuration explicit in the instance runtime model.
  OAuth application registration and credential verification now emit `null`
  when no public VAPID key is configured instead of an empty string, while the
  configured-key differential shape remains unchanged.
- Added fail-closed startup validation for an optional `WRITE_DATABASE_URL`.
  The bounded read-only catalog check rejects unsafe role attributes,
  ownership, membership, database/schema creation, writable defaults, and
  missing status/notification/outbox capabilities. Operational-schema ACL
  comparison ignores only the explicitly configured writer role. Guarded
  startup proves valid web/worker use and refusal before bind or work claim for
  unsafe writer configurations; the writer-privilege issue is archived.
- Re-ran `mise run check`, `mise run startup-integration`, `mise run
  worker-integration`, and the full `mise run differential` suite. The local
  gate passes 66 tests and the guarded differential suite passes 13/13.
- Completed the account recovery and mail slice. Durable reset and confirmation
  jobs now use AES-256-GCM envelopes in the PostgreSQL outbox, with SMTP lane
  delivery through lettre, explicit retryable transport failures, TLS/custom-CA
  handling, reply-to/return-path support, and the configured canonical origin.
- Added Devise-compatible PBKDF2-HMAC-SHA1 key derivation with
  column-specific salts and HMAC-SHA256 token digests for new SMTP-backed reset
  and confirmation records. Legacy SHA-256 and raw confirmation lookups remain
  available for existing Rails/Rust-owned rows. Reset tokens remain six-hour,
  single-use values; confirmation tokens expire after two days.
- Added `/auth/confirmation` and wired browser reset requests/updates to the
  durable outbox. Password reset now validates the 8..72-byte password policy,
  looks up the token before bcrypt work, clears sessions and stale sign-in data,
  and revokes both OAuth access tokens and pending authorization grants.
- Added `rustodon admin create-user` with normalized email/username validation,
  stdin password fallback, bcrypt credentials, generated RSA signing keys,
  initial `account_stats`, and transactional confirmation-mail enqueueing when
  SMTP is enabled. SMTP-disabled administration creates a confirmed user.
- Recovery refuses external-auth users, deletes push subscriptions while
  revoking OAuth access, and applies local-process IP/email reset throttles at
  Mastodon's 25-per-five-minute and 5-per-thirty-minute limits. Cross-process
  distributed throttling and live streaming kill events remain deployment-level
  follow-ups.
- Added mail encryption/retry, SMTP lane, CLI, digest, password-boundary, and
  guarded OAuth-grant recovery coverage. `cargo test --locked --all-targets
  --all-features`, Clippy with warnings denied, formatting, and cargo-deny all
  pass; fixture-backed integration remains gated by the unavailable restored
  PostgreSQL/Mastodon environment.
- Restored the pinned PostgreSQL/Mastodon fixture and removed the recovery
  integration gate. The guarded Rails-versus-Rust suite now passes 14/14,
  including an `admin_create_user` case that invokes both administrative CLI
  commands through stdin, verifies bcrypt credentials, `account_stats`, the
  encrypted confirmation outbox event, and confirmation success, replay
  rejection, and expiry. `mise run fixture-restore-verify` and
  `mise run fixture-verify` also pass.
- Live SMTP delivery remains unverified because no SMTP service is available.
  A local SMTP protocol integration now exercises the actual worker runtime and
  verifies the decrypted reset link in the received message. The refused-
  connection unit test still proves transport failures are retryable;
  cross-process reset throttling and streaming kill events remain
  deployment-level follow-ups.
- Added process-local browser login throttles matching Mastodon 4.6.5: 25
  attempts per client IP in five minutes and 25 attempts per normalized email
  in one hour. Focused limiter boundaries and the full 14-case differential
  suite pass; shared-store throttling remains a deployment-level follow-up.
- Added the Mastodon 5-per-10-minute client-IP throttle for OAuth application
  registration. Attempt windows now use fixed epoch buckets with bounded state,
  and throttled browser/API responses expose reset and retry headers.
- Added the first safe remote-fetching foundation: strict HTTP(S) URL checks,
  Mastodon-compatible private/documentation/reserved address rejection,
  mixed-answer DNS validation, fixed address pinning, redirect revalidation,
  disabled proxies, content-type checks, timeout bounds, and streaming body
  limits. ActivityPub profile parameters, exact `200 OK` responses, canonical
  JSON identity matching, compressed-response rejection, and hard configuration
  ceilings are tested. Signed GET transport and adversarial network fixtures
  remain open for the next federation milestone.
 - Added a typed WebFinger/ActivityPub actor resolver layer with canonical
  subject/origin checks, ActivityStreams context validation, supported actor
  types, safe endpoint shapes, profile-host binding, and actor-owned embedded
  public-key parsing. Exact remote account search and lookup can invoke it when
  a write pool is configured, enforce normalized domain allow/block policy,
  reuse fresh stored accounts, refresh stale WebFinger data, throttle
  process-local IP/handle misses, and transactionally upsert the minimal
  account identity by canonical actor URI. Same-origin redirects are rejected
  before cross-origin requests are made; actor field/key-count bounds,
   optional URL shapes, legacy `WHITELIST_MODE`, and current writer table/
   sequence capabilities are covered. Host-meta fallback and signed GETs now
   use origin-bound, redirect-re-signed transport. Actor public-key entries may
   be embedded or referenced; referenced key documents are bounded, fragment-
   safe, owner/ID checked, and reconciled into `keypairs` without discarding
   revocation or expiry metadata. A whole-resolution deadline and partial-key
   failure handling prevent key lists from multiplying latency. On-demand key
   refresh during inbound signature verification, media fetching, shared
   throttling, and adversarial federation transport fixtures remain open. Added
   bounded on-demand key refresh for inbound actor signatures: limited-mode and
   domain policy are checked before fetch, owner actor/WebFinger loopback is
   confirmed, stale keys retry once, fresh signatures verify before persistence,
   and incomplete key lists cannot delete existing key material. The full
   local, preflight, startup, and 14-case differential checks pass.
- Added the first ActivityPub inbox ingress boundary. Instance, shared,
  username, and numeric account inboxes now enforce an exact 1 MiB body bound,
  require remote HTTP signatures with signed SHA-256 digests, and enqueue opaque
  ingress jobs only after verification. A transactional 30-day idempotency
  marker deduplicates shared-inbox deliveries and retries, while ordering markers
  serialize jobs per signer. Web creates a runtime operational queue pool; the
  separate ActivityPub Note and relationship milestones will provide processing
  handlers without moving activity mutation or remote work into the web path.
  The local full gate passes 100 library tests plus all configured target tests,
  Clippy, formatting, cargo-deny, and fixture verification.
- Extended the bounded remote transport with signed ActivityPub JSON POSTs.
  Host, Date, Digest, and `(request-target)` coverage is rebuilt for every
  same-origin 307/308 redirect; request and response bodies remain bounded,
  compressed responses are rejected, and successful delivery statuses are
   accepted. The guarded worker integration now proves inbox deduplication and
   per-signer ordering against the restored fixture; HTTP-signature and inbox
   ingress issues are archived, while media fetching and activity processing
   remain separate open milestones.
 - Re-ran the restored-fixture worker integration and the complete 14-case
   Rails-versus-Rust differential suite after the transport changes. Both pass;
   the read-only REST surface now has production coverage for its listed routes
   and is archived alongside HTTP signatures and inbox ingress.
- Started outbound ActivityPub delivery with transactional local-status
   distribution intent. Push workers serialize Mastodon-compatible Create/Note
   activities, deduplicate remote shared inboxes into immutable per-inbox outbox
   events, enforce ActivityPub/domain policy and limited-federation mode, and
   deliver through signed bounded POSTs with retry/permanent classification and
   operational domain health. Private/direct audiences and reply targets are
   included; interaction, edit/delete, peer, and crash/retry delivery coverage
   remain open.
 - Expanded the durable ActivityPub relationship worker beyond Follow and Undo
   Follow. Remote Block and Undo Block now tear down relationships and preserve
   tombstone idempotency; blocked Follow requests produce signed Reject
   delivery intent; inbound Accept and Reject transition or remove URI-bound
   follow requests; and manual approval records a signed Accept outbox event in
   the same transaction as the local decision. Embedded and URI-only Undo,
   signer/domain checks, relationship locks, counters, and notification
   semantics remain in the write path.
 - Added restored-fixture worker coverage for blocked-follow rejection, Block
   and Undo Block, and inbound Accept/Reject transitions. The current checks
   pass: worker integration 10/10, schema integration 23/23, 115 library
   tests, all target tests, Clippy with warnings denied, formatting, and diff
   validation. Local relationship fan-out, actor Update/Delete, Note
   processing, and peer crash/order coverage remain open.
 - Added local-to-remote Follow/Undo delivery. Origin-aware relationship writes
   persist deterministic Follow URIs, enqueue signed shared-inbox delivery
   events atomically, cancel unsent or unleased Follow work when it is undone,
   and retain the original URI in the Undo payload. Delivery rechecks
   limited-federation and
   domain-block policy, while per-account domain blocks prevent local Follow
   creation. Restored-fixture schema coverage proves remote Follow/Undo payloads,
   duplicate outbox suppression, and cancellation behavior.
 - Tightened relationship convergence by removing FollowRequest notifications
   when inbound Accept/Reject decisions consume a request, adding remote-domain
   metadata to Accept/Reject delivery events, and avoiding notification writes
   for duplicate remote Follow activities. Local Block/Undo Block, Reject, and
   remove-follower delivery now use the same transactional signed outbox path;
   actor lifecycle, Note processing, and strict cross-worker ordering remain
   open.

## 2026-08-25

- Completed local-to-remote Block/Undo Block and Reject/remove-follower delivery
  with deterministic relationship URIs, duplicate-safe logical keys, unsent
  cancellation, remote-domain policy metadata, and signed per-inbox outbox
  events. Block teardown emits Undo Follow for removed outgoing relationships
  and Reject for removed incoming requests while retaining outgoing pending
  FollowRequests to match the existing Mastodon block behavior.
- Added a delivery-time state fence that suppresses stale queued positive Follow
  and Block activities after their relationship URI has been removed. Strict
  ordering across concurrent positive/undo deliveries remains a follow-up.
- The current verification gate passes: worker integration 13/13, schema
   integration 23/23, 119 Rust tests, all target tests, Clippy with warnings
   denied, formatting, cargo-deny, and diff validation. Outbound interaction
   fan-out and peer crash/order coverage remain open.
- Added signed actor Update/Delete handling. Updates validate that the embedded
   Actor is the verified signer, persist bounded profile/endpoints/field data,
   and ignore updates after deletion. Deletes sever relationships and related
   notifications, soft-delete remote statuses, clear the remote profile, and
   preserve idempotent tombstone-like suspension state. Restored-fixture worker
   coverage now passes 11/11, including update, delete, and non-resurrection.
- Added inbound Note Create/Update/Delete handling. Notes validate signer and
  object identity, preserve bounded raw HTML, audience visibility, replies,
  mentions, hashtags, remote media metadata, edit timestamps, and untrusted
  interaction counts, while URI locks, tombstones, duplicate suppression, and
  deleted-object fencing prevent resurrection. Restored-fixture worker coverage
  now passes 13/13, including duplicate Create/Delete, media metadata, and an
  Update after Delete.
- Added inbound Like/Undo and Announce/Undo handling. Interaction activities
  validate the remote actor/activity host, target only active local public or
  unlisted statuses, update favourite/reblog counters transactionally, create
  notifications, and use activity tombstones to fence duplicate and
  Undo-before-activity delivery. Media fetching, unresolved-parent reply
  repair, outbound interaction fan-out, and peer differential/order coverage
  remain open.
- Completed durable out-of-order ActivityPub reply repair against the pinned
  Mastodon `ThreadResolveWorker` behavior. Unresolved Note children remain
  `reply = true` and record one transactional Pull-lane job; resolution first
  checks exact and computed local status URIs, then performs bounded same-origin
  signed parent fetches, accepts Note/Create representations, validates object
  identity, resolves/upserts unknown parent actors, and attaches the child with
  Mastodon's carried reply-account rule and exactly-once reply counters. Inbound
  Notes now apply the Mastodon local-relevance gate, scalar/null `to` and `cc`
  audiences are accepted, and default write workers consume Pull jobs. The
  restored fixture passes 14/14, including remote child-first and computed-local
  parent cases. Media bytes, adversarial transport fixtures, and the full peer
  differential/order matrix remain open.
- Added a durable Pull-lane remote media job that uses the bounded SSRF-safe
  fetcher, enforces a 16 MiB response limit, applies the remote account's
  domain/reject-media policy, preserves Note text on failure, retries transient
  failures, and stores supported images through the exact Paperclip cache paths.
- Added tolerant Mastodon attachment-link parsing, description truncation,
  focal-point metadata, blurhash validation, media proxy retrieval, and
  Paperclip recovery for pre-existing partial files. The proxy now applies the
  existing REST status policy before fetching public or authenticated media and
  accepts Mastodon's no-style and trailing-style route forms.
- Restored-fixture worker integration now passes 15/15, including the media
  failure case; successful remote HTTP media fixtures, media lifecycle cleanup,
  adversarial transport, and the full peer differential/order matrix remain
  open.
- Moved local status mention, favourite, and reblog notification creation into
  transactional Core-lane outbox jobs. The worker resolves mentions and
  interactions through the existing idempotent policy kernel; restored-fixture
  coverage now proves 16/16 worker cases and 24/24 schema cases, including
  duplicate notification delivery and mention dispatch.
- Added origin-aware local-to-remote `Like`, `Announce`, `Undo Like`, and
  `Undo Announce` delivery. Payload builders match Mastodon's nested activity
  shapes, deterministic actor/status IDs, public/unlisted/private audiences,
  remote-domain policy, and unsent positive-event cancellation. Schema coverage
  proves the outbound interaction lifecycle; actor edits/deletes, media success
  fixtures, crash/order delivery, and peer differential coverage remain open.
- Added a deterministic feature-gated HTTP media fixture without relaxing
  production SSRF checks. The real media worker path now proves bounded fetch,
  GIF processing, cached Paperclip original storage, PNG small-style storage,
  metadata, and blurhash persistence; restored-fixture worker coverage passes
  17/17. Adversarial transport, cleanup/crash compensation, and peer
  differential coverage remain open.
- Added durable local status Update and Delete distribution. Edits emit
   Mastodon-compatible `#updates/<unix-seconds>` activities; deletes emit
   `#delete` Tombstones and are allowed through delivery after the status is
   soft-deleted. Distinct logical keys prevent edits/deletes from colliding with
   Create delivery, while deletion cancels only pending obsolete work. Worker
   coverage now passes 18/18 and schema coverage proves transactional Update and
   Delete outbox intent; actual peer delivery, crash/order behavior, and actor
   Update/Delete remain open.
 - Hardened status delivery after independent review: Update keys include the
   activity version, stale queued edits are coalesced before fan-out and fenced
   again before delivery, and delivery-time domain policy is rechecked even for
  older jobs without stored domain metadata. Status reach now follows
  Mastodon’s public/unlisted interaction rules for replies, reblogs, quotes,
  favourites, parent authors, and quote targets without expanding private,
  direct, or limited audiences. The feature-gated media fixture endpoint now
  refuses release builds; the aggregate suite passes 126 library tests,
   worker integration 18/18, schema integration 24/24, formatting, Clippy,
   cargo-deny, fixture verification, and diff validation.
 - Added real bounded-transport coverage for remote fetching. A local HTTP
   fixture now proves oversized responses, cross-origin redirects, and stalled
   responses fail closed through the production fetch implementation; existing
   DNS-address and canonical-identity tests cover rebinding and representation
   attacks without permitting internal network access.
  - Added a debug-only local inbox endpoint for deterministic signed ActivityPub
    POST tests. The durable worker now proves Host, Date, Digest,
    `(request-target)`, RSA signature, body delivery, and successful 2xx response
    handling; restored-fixture worker coverage passes 19/19 without changing the
    production DNS/address policy.
  - Moved follow, follow-request, inbound Note mention, Like, and Announce
    notification production into transactional Core-lane outbox events. The
    web and ActivityPub ingress paths now only mutate relationship/content state;
    the notification worker performs the idempotent policy-kernel write. Worker
    integration remains green at 19/19, including local FollowRequest dispatch
    and inbound interaction notification delivery.
  - Closed notification lifecycle races found in review. Deletion now uses the
    recipient advisory lock, cancels pending notification outbox events, and
    reconciles filtered notification requests. Already-dispatched jobs remain
    runtime-owned and are fenced by activity re-resolution. Local and remote
    mention edits preserve withdrawn mentions as silent rows, reactivate existing
    rows on re-mention, and cancel undelivered stale jobs instead of deleting history.
  - Kept the notification lifecycle compatible with the least-privilege web
    writer: the fixture grants only column-scoped UPDATE on mention silence and
    timestamps, preflight validates those capabilities, and the writer never
    mutates runtime-owned durable jobs. The focused notification differential now
    passes again after the unfollow/status-mention privilege regressions.
  - Added transactional outbound actor `Update` delivery for local profile
    changes. Versioned Push jobs serialize profile media and property fields,
    reach remote followers/reporters/recent contacts/enabled relays, deduplicate
    inboxes, and fence stale versions before creating signed delivery events.
   Restored-fixture worker coverage is now 20/20; schema and startup integration
   remain green.
 - Added local `POST /api/v1/reports` with `write:reports` authorization,
   transactional report/status/collection persistence, Rails-compatible category
   and omitted-forward handling, REST serialization, and durable `admin.report`
   notifications. Restored-fixture schema coverage passes 25/25, worker coverage
   passes 20/20, and the differential report case proves valid attached reports,
   silent-mention attachment, response parity, invalid-collection rollback, and
   invalid rule handling. Rule reads are granted to the least-privilege writer;
   target availability and duplicate rule validation now match the pinned peer.
   The generated URI intentionally follows the fixture's observed
   `<origin>/<uuid>` result from Mastodon's `URI.join` behavior.
 - Added permission-checked `admin resolve-report` and `--reopen` commands.
   Resolution updates and report audit rows commit together, reject disabled or
   non-moderating accounts, and are covered by the restored schema test and CLI
   help coverage.
 - Added permission-checked `admin delete-status`, reusing the local status
   deletion transaction for counters, reblogs, media behavior, tombstone
   delivery, and optional Paperclip cleanup; moderation deletions are audited.
  - Added administrator-only `admin reconcile-account-stats`, which repairs the
    denormalized account counters and latest-status timestamp transactionally and
    records the repair in the moderation audit log.
  - Added durable local boost distribution. Reblogs now enqueue Push-lane
     Announce work, unboosts cancel pending distribution and enqueue Undo Announce,
     original-status deletion preserves remote reblogger recipients, public status
     delivery reaches enabled relays, and private self-boosts inline the original
     Note. Mastodon fixture worker/schema integration passes 26/26 and 30/30.
  - Closed deletion fan-out and teardown gaps found in the federation review.
    Delete stream events now remove public, unlisted, and private entries from
    all active local followers while retaining direct/limited audience bounds;
    remote Note and actor deletion removes affected favourites, polls, and
     Favourite/Poll notifications with counter restoration. Worker and schema
     integration remain green at 26/26 and 30/30.
   - Added Mastodon-compatible local status activity routes at username and
     numeric account paths. Public, private, and direct `Create`/`Announce`
     responses now use verified ActivityPub or OAuth authorization; guarded
    differential coverage passes 18/18, including anonymous denial and signed
    private/direct reads.
  - Completed the local account deletion purge slice: self-service deletion now
    queues a durable 30-day maintenance purge, retains the actor identity and
    reported content, removes account-owned content and interactions
    transactionally, and preserves signed actor Delete delivery. Restored-fixture
    worker coverage passes 29/29, Mastodon schema coverage 30/30, and rollback,
    retry, poll/bookmark/pin retention, unsuspend cancellation, writer privilege,
     preflight, startup, and operational-schema checks pass.
   - Hardened outbound ActivityPub protocol boundaries: remote status mentions,
     status/account fan-out, report forwarding, remote reply forwarding, and
     follow acceptance now exclude protocol-0 accounts. Account-update delivery
     also fences microsecond versions against second-resolution activity IDs.
      Focused Rust tests pass; restored-fixture regressions and live peer
      convergence remain pending.
  - Added token-scoped authenticated-stream revocation. OAuth revocation,
    password resets, and browser-session deletion now record durable
    `kill:token` events; only the matching WebSocket closes, while account-wide
    suspension/deletion kills remain unchanged. The restored fixture proves
    revocation, reconnect rejection, sibling-token continuity, and cleanup
    without changing the fixture sequence.
  - Preserved OAuth consent requests across browser login with a validated
    local return target, added Rails-compatible CORS for OAuth/discovery
    surfaces, and switched OAuth client-secret comparisons to constant-time
    checks. Full local checks, the OAuth differential case, and operational
    streaming integration pass.

## 2026-09-03

- Hardened account purge filesystem cleanup. Claimed purge jobs now persist
  validated Paperclip paths before the database transaction, commit database
  cleanup first, and remove files afterward; failures retain the manifest for
  a lease-fenced retry. Restored-fixture coverage forces the filesystem failure
  after commit and proves recovery, while the durable queue test rejects stale
  manifest updates. Worker integration passes 35/35; hard power-loss and
  production disk-failure behavior remain unproved.

## Featured typed-entry diagnosis on d17bec9

No app bug/fix: v1/v2 instance metadata already advertises ten. Pinned UI delays
server fetch 3000 ms; old harness asserted too early. Focused `typed` adapter mode
waits for actual metadata and proves ten typed Add POSTs, reload/public-profile
persistence, warning at ten, ten UI DELETEs and reload removal. Fresh bounded NAS
PG14/7203 run passed; cleanup verified. New serializer/source-contract tests pass
on NAS, plus 5+8 offline harness tests. Local Rust blocked by existing macOS
rustix assumptions. Detailed provenance, failed test attempt, transfer deviation,
checks and exclusions are in tools/hashtag-controls-browser/README.md; sanitized
export target/featured-typed-d17bec9/evidence. No full/differential repeat, app
change. Independent parent review 293a7 approved code/harness with no blockers;
typed-limit follow-up and hashtag-controls parent are archived for ordinary local
acceptance. Closure reran 5+8 offline tests, Rust fmt and diff checks. Ten tags are
persisted in signed-in public-route response/DOM, not necessarily simultaneously
visible; warning correctness is after readiness only. Strict Rails FAIL 54/59 and
peer/Redis exclusions remain unchanged. No production push.

## Bounded instance activity aggregation after 3df2724

Created/indexed aggregation subissue before code. Dynamic exact UTC 4/24-week
unions now replace main's permanent zeroes, with shared singleflight TTL/date
cache, fail-closed 503s, endpoint-specific limited-federation privacy, and matching
initial metadata. Existing writer-backed maintenance prunes bounded member chunks
under bucket locks; no grants/migration/job/backfill. Rollout excludes today.
NAS7203/PG14 focused activity 7/7, maintenance handler 1/1, actual-main HTTP 1/1,
ordinary library 359 passing, serializers 20/20, all-target check and focused
strict Clippy pass. Full strict Clippy has pre-existing test-helper lints. Details,
failed attempts and evidence boundaries are in the aggregation issue. Browser and
independent parent review remain pending (subagent nesting limit); uncommitted,
no production or full-matrix claim.

### Activity aggregation review 1 follow-up

Removed activity availability coupling only from manifest/rules via the existing
instance projection loader. Actual main HTTP regression went red (rules 503), then
green (rules and both manifest aliases 200 during cold activity failure, while
count-bearing endpoints remain 503). Focused main test, strict Clippy, fmt and diff
checks passed on the bounded NAS7203/PG14 fixture. No TTL/privacy changes or extra
lanes; uncommitted for parent review 2, browser next.
