# Test gating and Mastodon coverage audit

## Scope and conclusion

Reviewed Rustodon production/test source at `f1ed300a42722631b9d42ef2935e9694728ce0d0`
(the subsequent audit-tracking commit changes documentation only). This was a
read-only audit: no new Rust/test/lint runs, live requests, preference writes,
replays, or implementation changes. Counts below reconcile source annotations
with the retained NAS `web-settings-nas/ordinary.log`, not a fresh execution.

**Most ignored tests are legitimately fixture-gated. The problem is incomplete
execution wiring and some missing independent behavioral assertions, not simply
too many `#[ignore]` attributes. Run the existing coverage reliably before porting
a large additional upstream suite.** Three concrete implementation mismatches
were also found; they need focused red/green regressions, not fixes hidden in this audit.

Reference sources:

- User-provided read-only `/Users/lainsoykaf/repos/pleroma-org/mastodon`, clean
  revision `761c61b42590a2fd91442fc15a0a7583e48bbea4`. Its version module declares
  **4.7.0-alpha.1**, despite `git describe` showing a 4.6.0-rc.1 ancestor. It is a
  useful test-discovery corpus, **not** the project's pinned 4.6.5 oracle.
- Canonical pin: `/workspace/rustodon/target/mastodon-v4.6.5`; existing Secunda
  counterpart `/home/lain/repos/rustodon/target/mastodon-v4.6.5`, revision
  `1440d55b139e39ec722c2a3db7f60b66cd889048`. Neither local checkout was substituted.
- For the two federation differences below, inspected the corresponding source
  and sanitized-update spec in the already-cached exact 4.6.5 harness image
  `sha256:696439e1ada71d0cf3d51d4d6a4744d6e40b57aafa64980b18f4d3b78230d0cf`
  on the NAS, without network access. No checkout was fetched or modified.
  Other proposed ports still require confirmation against that compatibility pin.

## 1. What “163 ignored” actually means

Latest recorded `cargo test --locked --all-targets --all-features`:
**431 passed, 163 ignored, zero failed**, across 29 targets. About 27% of the
compiled test cases were not executed by that command. Independent annotation
inventory found exactly 163 prerequisite-labelled ignored tests; no bare ignore,
`cfg_attr` ignore, or reason declaring a known-broken assertion was found.

| Ignored category | Count | Required execution environment |
| --- | ---: | --- |
| Worker suite, including nested lifecycle/profile/emoji modules | 71 | Restored DB, runtime/writer/owner roles, queues, synthetic transport, media files; serialized fixture use |
| Mastodon schema/read/write integration | 37 | Restored canonical schema and least-privilege reader/writer |
| Rails-versus-Rust differential cases | 23 | Separate fixture clones, Rails HTTP/DB oracle, media/Redis as applicable |
| Operational schema, streaming, cross-pool limiter/fetch-budget cases | 11 | Six operational + three streaming + two library DB cases |
| Focused HTTP regressions | 8 | Five standalone schema selectors plus account search, empty API reads and web settings library tests |
| Startup and canonical preflight | 6 | Five startup + one canonical preflight; real roles/config/process probes |
| Real Mastodon peer scenarios | 5 | Both applications/workers, fresh identities/origins, TLS transport/audit |
| Pinned upstream-source contracts | 2 | Verified read-only upstream checkout |
| **Total** | **163** | |

These are not 163 abandoned tests. For example, the preceding combined repair
run explicitly executed **71/71 workers**, and the settings repair executed the
focused save/reload test and **5/5 startup tests**. Conversely, a historical pass
on an earlier revision is not certification of the current tree or every suite.

`--all-features` does **not** run ignored tests. The only project feature is
`test-support` (`Cargo.toml:9–10`); it exposes test-only fault/transport helpers.
Without it, **49 source-declared tests disappear entirely**: 10 ordinary and 39
ignored. They are not reported as skipped. This is a static inventory, not a
working default-feature test result.

Do not run a blanket `cargo test -- --ignored`: these tests consume different
fixture environments, and worker reset helpers truncate shared operational
state (`tests/workers.rs:17551–17560`). The supported worker invocation uses
`--test-threads=1`. Many ignore descriptions say “starts … through Mise”, although
the Rust test actually consumes variables supplied by its shell harness.

One ordinary reported pass is intentionally subprocess scaffolding:
`peer_transport_child` exits when its child-mode variable is absent
(`tests/remote_peer_transport.rs:335–339`); its parent runs the substantive child
checks explicitly. This is not a second independent coverage result.

## 2. Execution and automation findings

### P1 — Recent regressions are not permanently wired into the normal fixture gate

`tools/mastodon-fixture:1010–1018` exposes only `v2_account_search` as a library
selector. Its default schema command (`:2837–2852`) runs five other HTTP targets
but **does not select account search**. There are **no permanent selectors** for
`web::api_empty_reads_tests` or `web::web_settings_tests`: their successful recent
runs required an untracked `sed`-modified harness.

**Recommendation:** add explicit named selectors and an aggregate HTTP fixture
command; include all three in CI. Each selector must verify that its expected
compiled test exists and executes. The existing peer runner's `--list --ignored`
check (`tools/federation-peer-smoke:30–34`) is a useful pattern. The differential
runner does whitelist names (`tools/mastodon-fixture:2170–2180`), but a stale
whitelist/test-name mismatch can still produce Cargo's successful zero-test run.

### P1 — Fresh worker CI lacks an actual fixture prerequisite

The worker job (`.github/workflows/ci.yml:84–108`) checks out Rustodon and installs
tools/Podman, but never obtains upstream source. `worker_test`
(`tools/mastodon-fixture:1241–1314`) checks static fixtures/images, not source assets.
Six tests read `attachment.gif`, `avatar.gif` or `attachment.jpg` under
`target/mastodon-v4.6.5/spec/fixtures/files`.

This is more than conjecture: the earlier clean NAS run passed **65/71** and failed
six cases with ENOENT until those three files were extracted from the exact pinned
image. CI configuration has the same missing prerequisite on a fresh checkout.
This audit did not inspect hosted CI runs and does not claim their observed status.

**Recommendation:** explicitly provision/checksum these exact assets before
workers—prefer a small verified fixture dependency over requiring an entire
checkout solely for three media files. Retain the full pinned checkout for the
separate source-contract gate. Missing prerequisites should fail before compilation
with a targeted diagnostic, not six opaque late failures.

### P1 — The supported test-profile matrix is incomplete

- Plain `cargo test` has a known compile defect: ignored
  `smtp_acceptance_before_job_ack_is_retried_with_the_same_message_id`
  (`tests/workers.rs:477–496`) calls feature-only `Queue::with_complete_fault`
  (`src/jobs.rs:342–344`) without the matching gate. Ignored bodies still compile.
  This was previously encountered during implementation; no new run was made here.
- Release/all-feature synthetic transport tests have a source-level mismatch:
  `bounded_transport_fixtures_fail_closed` (`src/remote.rs:3144–3161`) expects
  `BodyTooLarge`, but its test transport returns `Client` when debug assertions
  are disabled (`:613–614`). Other positive transport fixtures have the same
  debug-only prerequisite. This release-test failure is inferred, not newly run.

**Recommendation:** fix the individual missing test gate, align positive transport
tests with debug-only helpers, and keep explicit production/release negative
capability tests. Never enable a production SSRF bypass to make tests pass. Require
both a default-feature compile/test gate and the feature-enabled debug suite;
production release builds remain separately checked.

### P1/P2 — CI covers only part of the integration contract

The checked-in workflow schedules ordinary checks, pinned-source contracts,
schema/operational integration, workers, and selected differential cases.
It has **no startup, full preflight, cutover, browser, or real-peer job**. Startup,
preflight, cutover and browser commands exist in `mise.toml`; real-peer scenarios
have the separate `tools/federation-peer-smoke` runner. `mise run check` only depends on formatting,
Clippy, ordinary tests, dependency policy and static fixture verification.
`mise test`'s “Run all Rust tests” description is therefore misleading.

`tools/ci-differential:8–15` selects **six distinct cases out of 23** (seven
invocations because media-root mode is repeated), omitting, among others, browser
recovery fences and reauthentication-limit cases. Shell/Python harness regression
scripts are also not selected by this workflow. Add fast harness tests and
high-risk auth/startup/HTTP suites to the required lane; schedule broader
Rails differential, browser/cutover and peer matrices in bounded separate lanes.

The recorded real-peer evidence is only an earlier public push smoke. Expanded
privacy, Note lifecycle, profile and interaction scenarios are implemented but
not proven executed; Pleroma's build/peer evidence remains separately incomplete.
Ordinary peer helper tests and synthetic workers do not change that fact.
`tools/federation-peer-smoke:4–6` also enforces a Secunda-only host/work-directory
guard, and `:22` hardcodes that host's reference path. NAS execution requires a
narrowly authorized guard/path adaptation while preserving source/fixture
verification and isolation; provisioning source alone is insufficient.
Host-policy/runbook text still says Secunda-only despite the user's NAS exception.

## 3. Concrete behavioral mismatches found during the audit

These are **source-confirmed findings, not newly executed regressions or live
incident diagnoses**. Fix each in a separate red/green, independently reviewed
change; do not fold them into harness cleanup.

### P1 — Explicitly mentioned remote direct messages are classified as limited

`remote_note_visibility` (`src/mastodon/write_repository.rs:11963–11985`) can return
0/1/2/4, never direct visibility 3. Remote Create stores that result; later mention
processing does not distinguish explicit mentions from silent recipients for
classification. The pinned Mastodon Create path (`app/lib/activitypub/activity/create.rb:126–150`)
keeps explicit-mention direct messages direct and changes them to limited only
when silent audience recipients are added.

This is observable beyond an enum difference: Rust serializes visibility 4 as
`private`, not `direct` (`src/mastodon/rest/serializer.rs:2002–2007`), and direct
conversation persistence selects visibility 3 (`src/mastodon/write_repository.rs:10080–10097`).
This is **not evidence of public disclosure**.

The existing unexecuted peer privacy scenario already tests a directly mentioned
recipient in both directions and requires receiver DB visibility 3
(`tests/federation_peers.rs:439–459,490–535`). It should expose the mismatch; do not
write a duplicate peer scenario. Port the smaller upstream explicit/silent-audience
matrix for fast diagnosis, then run that existing peer scenario. Add a receiver
REST visibility assertion; current receiver REST checks verify access/URI only.

### P1 — Sanitized-equivalent inbound Updates manufacture edit side effects

After timestamp checks, `src/mastodon/write_repository.rs:4304–4333` updates
`edited_at` without comparing meaningful content; `:4381–4398` records update
notifications/stream events. The exact pinned update service uses significant
changes to gate edit timestamps, snapshots and broadcasting, with an explicit
sanitized-HTML-only regression (`spec/services/activitypub/process_status_update_service_spec.rb:59–79`).

Port that example with a **genuinely newer** timestamp and two inputs that sanitize
to the same output. Assert unchanged edit state and absence of edit-side effects.
Retain existing older/implicit-update tests; do not treat a stale timestamp that
gets rejected early as proof of semantic equivalence handling.

### P1/P2 — Cached media-proxy derivatives can have the original MIME type

`cached_remote_media_response` (`src/web.rs:2600–2638`) opens the small-style file
but changes Content-Type only for GIF. Paperclip small styles are PNG for video
and JPEG for converted-image types (`src/paperclip.rs:832–839`). An existing MP4
small preview can therefore return **PNG bytes labelled `video/mp4`**, with the
analogous converted-image mismatch. This finding does not depend on upstream
version differences and is not the already-repaired raw-GIF serializer bug.

The existing proxy differential checks status and permits redirect-versus-200
representation differences; it does not assert MIME/decoded bytes. Add a
cached-original/cached-small HTTP matrix and derive Content-Type from the actual
served style consistently. No live attachment was fetched or altered in this audit.

## 4. What to port from Mastodon next

Paths in this table are relative to the authorized upstream checkout. Revalidate
against pinned 4.6.5 before adopting contracts not already checked above. Port
**behavioral examples and adversarial input matrices**, not Rails model callbacks,
Sidekiq implementation details or unsupported 4.7 features wholesale.

| Priority | Concrete upstream examples | Existing Rust coverage / useful additional assertion |
| --- | --- | --- |
| P1 | `spec/lib/activitypub/activity/create_spec.rb:292–299,442–482` | Limited/silent lifecycle exists; add explicit-vs-silent direct classification matrix and run existing peer privacy case. |
| P1 | `spec/services/activitypub/process_status_update_service_spec.rb:59–79,230–241` | Older/implicit and same-second updates exist; add sanitized-equivalent newer update and no-edit-side-effect assertions. |
| P1 | `spec/models/media_attachment_spec.rb:171–205`; `spec/requests/media_proxy_spec.rb:9–59` | Raw-GIF serializer, Paperclip and JPEG CRUD already exist. Connect DB loader → JSON type → original/preview HTTP MIME → decoded bytes, including cached video/converted-image derivatives. Preserve Rust's supported raw-GIF representation; do not silently require transcoding. |
| P1 | `spec/requests/api/v1/media_spec.rb:8–68,149–244`; `spec/requests/api/v2/media_spec.rb:9–62` | Extend happy-path JPEG CRUD and existing attachment-race tests with processing state × owner/other × attached/unattached HTTP outcomes and nonmutation on rejection. Mark unsupported async formats explicitly. |
| P1 | `spec/requests/api/web/settings_spec.rb:9–35` plus `app/javascript/mastodon/actions/settings.js:22–32` | New Rust HTTP save/reload coverage already exceeds the small Rails request spec. Wire it in; add actual browser action → debounced PUT → reload, observing failing API responses instead of only a mounted shell. Do not re-port the same PATCH test. |
| P1/P2 | `spec/lib/activitypub/activity/create_spec.rb:118–156` | Current reply repair inserts the parent before pull processing. Add parent fetch 500 → child retained/unresolved → parent arrival → home eligibility, exactly one logical mention notification per mentioned status, with no duplicate child notification after repair/replay. |
| P2 | `spec/lib/activitypub/activity/delete_spec.rb:30–50` | Existing remote-original/local-reblog topology asserts distribution outbox enqueueing. Drain distribution/delivery and assert the correct withdrawal, then duplicate Delete/replay without extra logical effects. |
| P2 | `spec/requests/api/v1/accounts/credentials_spec.rb:48–129` | API text/defaults and browser picture uploads already exist. Add mixed GIF avatar/JPEG header credentials request, text-only preservation, one-slot replacement/removal and no partial mutation on rejection. |
| P2 | `spec/requests/api/v2/search_spec.rb:23–91` | Existing v1/v2 tests are extensive but often use one Rust route as the other's oracle. Add fixed independent exact-ID ranking/following expectations. Anonymous resolve behavior differs in the alternate checkout; do not call that a 4.6.5 defect without checking the pin. |
| P2 | `spec/lib/activitypub/activity/follow_spec.rb:104–203` | Existing replacement IDs/stale Undo and local preference tests are substantial. Add inbound replacement Follow IDs across accepted/pending × locked/silenced/muted states, with exact Accept/Undo identity. |
| P2 | `spec/requests/statuses_spec.rb:10–190`; `spec/requests/catch_all_route_request_spec.rb:5–21` | REST authorization is covered; extend actual HTML profile/status routes for owner/follower/outsider, private/direct, blocked/suspended, boost redirects and cache/Vary behavior. Shell success alone is insufficient. |

Further small candidates: serializer highlighted-role suspension with deliberately
nonempty role input; trusted versus untrusted counts with different values;
read→interaction→reload projection freshness; malformed nested login/profile
parameters with valid CSRF and no mutation. Inline followers-collection Announce
shape and Collection-Synchronization delivery headers are lower-priority upstream
contracts to check, not newly established provenance or delivery failures.

## 5. Recommended execution order

1. **Make existing coverage trustworthy:** repair feature/profile gates, provide
   pinned worker assets, permanent HTTP selectors, nonzero-selection guards and
   clear prerequisite diagnostics. Add focused regressions/startup/security and
   fast shell/Python harness tests to automation. Update NAS/source run instructions.
2. **Three small bug/regression changes:** direct-vs-limited classification,
   meaningful inbound edits, and derivative MIME. Keep them independent of runner
   changes; preserve privacy/provenance/least-privilege fences.
3. **Run the already-written peer matrix** on the authorized NAS with verified
   prerequisites. Re-run public plus privacy/notes/profile/interactions on the
   combined revision; retain per-scenario received-state/privacy evidence. Complete
   Pleroma separately rather than calling Mastodon-only success universal parity.
4. **Port the next high-value matrices** from section 4, starting with media state
   and browser action/save/load. Use independent pinned expectations and assert
   side effects, not only HTTP success. Prefer hermetic units for pure logic and
   explicit disposable-DB/peer lanes for integration; retain safe resource bounds.

No blanket removal of ignores, bulk Rails-spec translation, new dependency,
feature implementation or live remediation is proposed as part of this audit.

## Independent review

Read-only synthesis review `09764a20-7962-4bcc-aeef-f85bbe47c1c5` found no material
blocker. Applied its corrections: real-peer runner is not a Mise task; NAS use
requires adapting the executable host/work-directory guard as well as the source
path; reply-notification expectations are per mentioned status, not one total
notification after both child and parent arrive. Review did not rerun tests or
inspect live services.
