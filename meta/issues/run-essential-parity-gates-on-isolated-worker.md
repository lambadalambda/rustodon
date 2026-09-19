# Run essential parity gates and peer tests on an isolated worker

## Summary

Execute all builds and tests for the essential-parity fixes on an isolated worker, leaving the local machine and live Rustodon/Pleroma instances untouched.

## Requirements

- Use isolated task workspaces and a rootless container engine on an isolated worker for fixture/integration and peer tests.
- Reuse the existing read-only pinned Mastodon source checkout (revision `1440d55b139e39ec722c2a3db7f60b66cd889048`); do not retrieve another copy when this one exists.
- Never synchronize local instance credentials, backups, or untracked remote configuration.
- Add reproducible Mastodon-to-Rustodon and Pleroma-to-Rustodon peer coverage where useful, checking ingestion, identity, visibility, notifications, and lifecycle convergence rather than HTTP acceptance alone.
- Keep review observations R17/R18 (queue performance) deferred.

## Acceptance Criteria

- Focused red/green regressions, independent reviews, and the applicable aggregate quality gates run on an isolated worker for the implemented fixes.
- Containerized peer evidence distinguishes passed activities/directions from remaining unproven cases.
- Commands, prerequisites, isolation, cleanup, and remaining blockers are documented for repeatable runs.

## Notes

- Historical gates ran on an isolated Linux x86-64 worker with Rust/Cargo 1.97.1
  and a rootless container engine. Private host provisioning and configuration
  details are intentionally omitted.

## Cached fixture images

- Combined gates hit Docker Hub's unauthenticated pull limit after the ordinary
  all-target/all-feature tests passed. The fixture re-inspects registry indexes
  and pulls on every run even when the exact immutable child is already present.
- Repair the harness to reuse a locally inspected child only when its digest,
  operating system and architecture match the checked-in pin. Retain registry
  index verification and pinned pulling for cache misses; reject mismatches.
  Add offline mock regressions and prove a real restored suite using the cache.
- Cache repair RED: the offline cached-match shell test failed before the change.
  GREEN: all seven mocked cache/mismatch/cold-pull cases, POSIX shell syntax,
  and a real restored worker run using cached pins passed. Independent source
  review found no blockers. No credentials, alternate registries or floating
  images were used.

## Combined security and ingestion checkpoint

After integrating R01/R02/R03/R04/R06/R13 on the isolated worker:

- Restored worker suite: **53 passed**.
- Schema-read suite: **37 passed** plus saved-status HTTP lifecycle **1 passed**.
- Ordinary all-target/all-feature tests, formatting and warnings-denied Clippy
  passed. Ignored database suites were not counted as ordinary test passes.
- Both recovery-fence and shared-reauthentication-limit differential cases passed
  again on the integrated tree.
- Historical external run artifacts are not in the repository.
- This checkpoint is not a final parity gate: audience/lifecycle/client repairs
  and real peer acceptance remain in progress.

## Latest completed gates and outage

- The integrated R05/R07 checkpoint passed **57 restored-worker tests**, ordinary
  all-target/all-feature tests, formatting and warnings-denied Clippy. Historical
  external run artifacts are not in the repository.
- All seven `tools/ci-differential` invocations passed at that checkpoint; the
  historical external run artifact is not in the repository.
- R08/R09/R10 and R11/R12/R14/R15 were subsequently integrated from independently
  reviewed, remotely tested topical commits. Initial real Mastodon public push
  smoke and test-only transport also passed remotely in their isolated task.
  These are not a substitute for a final combined-tree run.
- Remote access to the isolated worker then failed. The attempted atom-tag parser/worker RED
  timed out; its logs are unavailable and its result is unknown. Do not count it
  as an executed regression pass or failure.
- The user requested continuing without that isolated worker for now. Continue source edits,
  Git and independent source review only: **no local builds, tests, formatting,
  lint, or container fallback**. Atom-tag repair, expanded peer scenarios and v2
  account-search followup require execution when access returns.
- Pleroma remained blocked by Docker Hub's anonymous pull quota; no pinned Pleroma
  build or peer result is claimed.

### Resume verification (isolated worker only)

1. Reconcile only the recorded timed-out run's processes/resources before starting
   another fixture. Do not prune other task containers, images or volumes.
2. Sync tracked source and new tests, excluding ignored instance/config/target
   content. Keep the shared pinned upstream source read-only.
3. Run formatting; ordinary all-target/all-feature tests and warnings-denied
   Clippy; fixture worker and schema-read suites; both recovery/reauthentication
   differentials; `tools/ci-differential`.
4. Run the documented peer-transport enabled/disabled gates and actual Mastodon
   smoke/privacy/lifecycle scenarios against the integrated source. Preserve
   received-state/privacy evidence and record every blocked scenario explicitly.
5. Only after quota reset, retry the exact missing Pleroma bases through the
   documented guarded build procedure, then bootstrap and exercise the shared
   scenarios. Source/build guard tests alone are not Pleroma federation evidence.

R17/R18 and the separately noted stress/legacy-history limits remain deferred.

### Source-only continuation integrated

- Atom metadata repair: `a3fe48a`; v2 account-search reuse: `0d513f5`.
- Expanded peer test source is merged in separate privacy, Note lifecycle,
  profile Update, and Like/private Announce Undo commits. Independent source
  reviews approved after corrections; none of the expanded scenarios is claimed
  green. The historical public smoke passed on its earlier source revision, not
  on this final combined tree.
- Add `tools/mastodon-fixture schema-read-test v2_account_search` explicitly to
  the resumed gates; it is a named opt-in library fixture regression.
- Run all five peer scenarios (`public`, `privacy`, `notes`, `profile`,
  `interactions`) and current audit/multipart unit regressions as documented in
  `docs/federation-peer-smoke.md`. Each scenario starts fresh peers.
- Source-only work is complete for this continuation. Runtime acceptance remains
  blocked on isolated-worker access and, separately, the Pleroma base-image quota.

## Combined final-batch verification plan (b64e94d, 2026-09-19)

- Exact application tree: `b64e94d263cfbc392fc0e3a17830aae7802251e6`, initially clean.
  Only `git archive` tracked source is transferred. Documentation recording does
  not change the tested application. No deployment, push, live access or keys.
- Native Linux x86-64 NAS workspace `rustodon-combined-b64e94d-alice`; reuse
  verified tools image `7203e0222e2b…` and compile caches, not copied artifacts.
  Each heavy command serial: 4 CPUs, 8 GiB, 512 PIDs, 1800s outer/1770s
  container deadline; disposable internal PG14 network, dedicated DB/roles.
  Inspect/remove only this task's containers/networks/volumes after runs.
- Ordinary lane expanded serially (not concurrent `mise check`): `cargo fmt
  --check`; `cargo clippy --locked --all-targets --all-features -- -D warnings`;
  `cargo test --locked --all-targets`, `--all-features`, and
  `--release --all-targets --all-features`; `cargo deny --locked check`;
  `tools/mastodon-fixture verify`; `tools/verify-worker-media`;
  `tools/check-harnesses` on Linux.
- Then `tools/pinned-source-contracts` with verified clean 1440d55b readonly;
  `cargo test --locked --all-features --test media_processor -- --ignored
  --nocapture --test-threads=1`; named `operational-schema-test`, standalone
  bootstrap, `schema-read-test`, `worker-test` fixture lanes where launcher
  prerequisites permit; explicitly report any narrower actual selectors.
- Targeted `differential-test` media/tag selectors, existing combined browser
  controllers (agent-browser, schema 6), then named `peer-public` if cached
  prerequisites permit. Derive exact selectors from source before invocation.
  Five known Rails tag message divergences remain strict failures, not normalized.
- Overall working envelope approximately two hours with warm caches; retain first
  failures and continue independent safe lanes where useful. At most two blocker
  adaptation attempts, no app fixes or assertion removal. Missing prerequisites
  and unexecuted gates stay explicit, not renamed passes. Browser cleanup exit
  must be inspected independently of inherited workload exit status.
- Independent review delegation unavailable in this nested session (depth limit);
  any substantive harness/source adaptation must remain for parent review.

### Actual result: paused at fixture-adaptation boundary

Evidence: `target/combined-b64e94d-evidence/evidence/` and
`/srv/workspaces/rustodon-combined-b64e94d-alice/evidence/` on the NAS.
`results.txt` retains every invocation's exit; logs and task-only launchers are
retained. All **6485** archived tracked-file SHA-256 values match b64e94d after
execution. Archive SHA-256:
`d5652f3d480a3d11864003df18ca787c27061590e3f339fee4162cce50195334`.

- Passed: formatting; complete default-feature all-target ordinary tests;
  Linux `tools/check-harnesses`; static fixture and vendored worker-media checks;
  named real media processor (**3/3**); full `tools/pinned-source-contracts`
  (**12/12**). Existing NAS reference checkout at
  `rustodon-representations-41a771e-alice/source/target/mastodon-v4.6.5`
  verified clean at 1440d55b and mounted read-only. An initial archive-only
  source attempt passed 11/12 but failed the explicit `.git` prerequisite;
  that failure is retained, not counted as the final source result.
- Strict all-target Clippy failed: test length at `tests/mastodon_schema.rs:8971`,
  two pass-by-value warnings at `src/paperclip.rs:2937`, and three
  items-after-statements warnings at `src/worker/local_uploads/tests.rs:338`.
  No baseline checkout was run; no claim these were newly introduced.
- All-feature debug/release ordinary attempts failed because external `kill`
  is absent in the tools image (`media_processor` cancellation test). Initial
  ordinary attempts also hit read-only `/workspace/target`; a task-owned writable
  scratch mount resolved that and the default suite passed. No assertions changed.
  `cargo deny --locked check` blocked: cargo-deny absent.
- Task-only derived image `484b37b48bddbffc0d4e57c159be784cb264db01c17d710da204fcafc5c0cf50`
  adds Python/Node/Git to immutable 7203 base; base unchanged. Initial `podman
  build --cpus` was unsupported; bounded task container install/commit succeeded.
  No further tool-image repair (external kill/cargo-deny) was attempted.
- Focused restricted-PG attempts, **not full named schema/worker gates**:
  `activity::` ignored library selector: **6 passed, 1 failed**
  (`recording_waiting_for_cleanup_recreates_no_orphans`, `Elapsed(())`).
  `worker::activity_tests::` ignored selector: **1 passed**.
  Setup helper `local_upload_http::local_rich_upload_schema_setup` passed on fresh
  restores, but does not prove upgrade or HTTP acceptance.
- Fixture setup was incorrect: initial owner was superuser; second owner was
  non-superuser, but created runtime/writer roles retained default `INHERIT`.
  Source requires `NOINHERIT` (`tools/standalone-bootstrap-fixture:227–231`,
  `src/operational_schema.rs:1236`). Role diagnostic retained. Consequently
  `instance_activity_upgrade_from_five_preserves_history_and_grants`,
  `operational_schema_lifecycle_is_isolated_and_idempotent`,
  `local_upload_http::local_rich_upload_http_lifecycle`, and
  `standalone_bootstrap_installs_and_verifies_exact_baseline` failed privilege
  validation. These are **runner failures, not established app regressions**.
  Initial failed upgrade dropped activity tables; subsequent independent storage
  tests used a fresh restore. Stop after the two setup variants; do not broaden
  into application fixes. Resume should use exact named-lane role provisioning,
  not these incomplete manually created roles.
- Actual named `RUSTODON_PEER_SMOKE=1 tools/federation-peer-smoke public` reached
  verified static fixture/cache-only pinned image prerequisites, then stopped
  at `cargo: command not found` on the worker host, before resource creation.
  **No peer activity/direction passed**; tunnel/secrets were not needed/requested.
- **Not run:** full operational/schema/worker/standalone launchers, complete
  v4→5→6 upgrade proof, focused main-process activity/readiness and remaining
  integrated HTTP slices, Rails media/tag differential, combined browser, and
  further peer scenarios. Known 54/59 tag differential remains historical, not a
  current-tree result. No browser launched, therefore no browser teardown pass.
- Independently confirmed task test/build containers absent; removed task PG
  container with its exact anonymous volume and internal network; verified exact
  volume absent and retained final engine inventories. Workspace/logs, derived
  tools image and pre-existing shared caches retained intentionally. No broad
  prune, production access, push, deployment, key changes or source/harness edits.

Issue remains **open**. This is partial final-tree evidence, not a green combined
milestone. Parent must review any future harness adaptation; nested independent
review was unavailable. Verification-only work has no TDD implementation cycle.
