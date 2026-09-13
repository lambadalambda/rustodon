# Exercise delayed browser settings save and reload

## Summary

Extend the existing browser gate beyond shell mounting with a real frontend setting action, debounced PUT, persisted state and reload. Preserve deliberate authentication/missing-resource errors while failing unexpected API responses.

## Acceptance Criteria

Use agent-browser on an isolated Linux fixture only. Confirm pinned 4.6.5 frontend action timing/contracts; demonstrate a failing behavioral baseline and browser green. Do not duplicate the already passing direct HTTP PATCH test or touch signed-in live sessions.

## Notes

- Subissue of [selected matrix ports](port-mastodon-media-and-browser-matrices.md).
- Tests first; separate topical implementation and independent review.
- In progress: phase 2 implementation prepared after the parent recorded the phase-1 NAS red baseline; awaiting parent-run green verification.
- Scope: `tools/rustodon-browser-smoke`, its existing shell test, and this issue only. Parent owns fixture/CI/provisioning, all execution and commits; no live sessions or private data.
- Loaded the agent-browser discovery skill and repo-issues skill. Before actual browser commands, parent must load `agent-browser skills get core` from the installed NAS version.
- Read-only pinned source: `/Users/lainsoykaf/repos/rustodon/.local-instance/audit-reference/remaining/app/javascript/mastodon/actions/settings.js` and `spec/requests/api/web/settings_spec.rb` (provided cached `696439...0cf`). Canonical `/workspace/rustodon/target/mastodon-v4.6.5` and Secunda `/home/lain/repos/rustodon/target/mastodon-v4.6.5` unavailable; no fetch.
- Contract: frontend `changeSetting` dispatches a PUT with `{ data }`, excluding `saved`; debounce is 2000 ms, leading and trailing. Existing request spec covers direct PATCH, not the browser behavior.
- Existing bundled source maps expose stable Home checkbox IDs `setting-toggle-home_timeline-shows-reblog` and `setting-toggle-home_timeline-shows-reply`. Use visible associated labels for real UI changes. Wait for the leading response **and its `SETTING_SAVE` completion handler** before the second change, still inside its 2-second window, to avoid a leading response marking a newer change saved. Assert the trailing PUT payload, then fresh bootstrap and checkbox values after reload.
- Phase 1 ready: extended the existing offline shell mock with ordered UI-click, leading/trailing observation, reload/persistence, and API-observation checkpoints; preserved and now require the CSRF/login/profile/logout sequence. Inject false, empty, quoted-true, malformed, and command-error results at every authenticated gate; cover anonymous API observation with empty credentials and require cleanup.
- Checkpoint seam: eval comments `/* rustodon-smoke: NAME */` return literal booleans. Names: `api-observer-ready`, `settings-ui-ready`, `settings-leading-observed`, `settings-trailing-observed`, `settings-reload-persisted`, `api-responses-acceptable`. These tests verify shell orchestration/fail-closed handling only, not JavaScript semantics or an invented CLI network schema.
- Independent phase-1 review completed; fixed its two findings by recording curl operations in the ordered transcript and clearing anonymous credentials. Follow-up review found no blocking concerns. Production script was unchanged during phase 1.
- Parent-reported NAS red baseline: **95 failures** from missing action/save/API gates; log `/srv/workspaces/rustodon-audit-green/logs/browser-save-red.log`. This evidence was reported by the parent, not executed or inspected here.

## Phase 2 handoff

- Implemented real Home boost/reply label clicks, capture-phase input click timing, and observation-only XHR hooks. Require two successful PUTs carrying the selected boolean settings and excluding `saved`; yield past the leading response's completion task before the second action. No fabricated Redux dispatch, module import, direct settings HTTP mutation, or fixture/CI changes.
- Bounds: 2100 ms initial settling allowance; leading send within 500 ms of its input click; second click within 1800 ms of the first and after leading completion; trailing send 1900–6000 ms after the second click. Leading/trailing polls are capped at 1200/6500 ms; CLI wait/eval processes have a 20-second Linux `timeout` guard. Slow runs fail rather than silently exercising two leading saves.
- After a bounded API drain, recheck the two-save contract, reload the document, and assert both fresh bootstrap values and rendered checkbox states. Only two target booleans and a document timestamp cross reload through task-scoped session storage; remove that marker after checking. This does not prove the absence of arbitrarily delayed future writes.
- API observation uses buffered Resource Timing `responseStatus`, not an assumed CLI response envelope. Reject unexpected non-2xx, unavailable status, cross-origin API resources, or a possibly truncated 250-entry resource buffer. Explicit credential-free read-only probes require exact 401/404 outcomes **and** matching observed resource entries; they do not exempt frontend failures on the same paths without the dedicated query marker.
- Per-document fetch/XHR accounting requires no pending tracked requests and 500 ms of quiet, within 8 seconds; fetch tracking lasts through cloned body consumption and transport failures are sticky. Install hooks before tested SPA/control actions in each loaded document.
- **Observation limitation:** buffered completed startup responses are audited, but hooks start after initial page loading. Pre-installation fetch/XHR still pending in that interval can escape observation. This is not perfect document-start interception; closing that gap requires a verified installed-CLI pre-navigation capability, not a guessed command.
- Expanded the existing offline test with Node VM checks of actual production snippets: eval syntax, response classification/probe consumption, leading/trailing payload and timing negatives, completion-task ordering, reload persistence, request accounting, bounded drain and quiet resets. No local execution; `node` and Linux `timeout` are required on NAS.
- Independent phase-2 production and test/JS reviews approved the final handoff after focused fixes (click ordering, fresh pending accounting, hook placement, post-drain contract recheck, and stronger virtual-time/probe fixtures). No remaining blocking review findings; the startup observation limitation remains explicit.
- Parent next: sync only the three scoped files and run the isolated browser cutover through the parent-owned API-service wrapper. CLI/browser compatibility and real action behavior still require runtime evidence before claiming browser green.
- No local test/build/fmt/browser workloads, NAS/SSH execution, or commits performed here. Issue remains open pending real-browser green evidence.

## Verified CLI compatibility follow-up

- Parent reports **shell + Node JS checks green** on NAS with Node 24.15.0. Actual `agent-browser@0.31.1` and Chrome are installed in tool image `dd8b417b66aaea2b8105380ea9bc324cfa114b62e661c6ee1b302710e72e31c1`; parent loaded `agent-browser skills get core` before usage.
- Real cutover stopped immediately at the pre-existing unsupported `--pin-tab` flag. Removed that flag only from the browser invocation; retain verified `--session` and `--args`. Append the script PID to the existing worktree session name so each concurrent worker invocation owns its single-tab session rather than sharing the worktree's active tab. All commands and cleanup retain that same run-specific name; no replacement tab flag or new CLI command is guessed.
- Regression mock now rejects the old invocation prefix, requires a numeric run suffix and consistent session on every operation, asserts cleanup targets that exact session, and checks independent smoke invocations use different sessions. API observation and frontend action logic are unchanged.
- Independent read-only review found no blocking CLI-isolation or test findings. Parent subsequently reports the CLI-adjusted shell/JS checks green. No commands, tests, browser/NAS/SSH workloads, or commits executed here during that follow-up.

## Bounded progress diagnostics follow-up

- Parent reports real cutover exits **124** after schema readiness with no browser progress in `browser-cutover-cli-green.log`. This does not establish a Chromium/capability/sandbox cause. Previously session discovery, initial `network requests --clear` (which may initialize the browser), `open`, and cleanup had no wrapper deadline or progress output; only waits/evals were bounded.
- Added fixed-label begin/end records to stderr for every existing CLI boundary, starting with `session-id`, then `network`, `open`, `wait-load`, `wait-predicate`, and `eval`. Each end record reports the original numeric status. No argv, session names, host/resolver arguments, URLs, eval source, form values, or cookies are included in these trace records. Fill stdout is suppressed; existing CLI stderr remains passthrough, not a guaranteed sanitized channel.
- All CLI boundaries, including launch/discovery and cleanup, now use the existing Linux `timeout` dependency with a 20-second deadline and five-second TERM-to-KILL grace. A forced kill can report 137; 124 identifies a timeout result, not its root cause. Cleanup diagnostics remain visible and do not replace the main result. No capability, sandbox, Chromium argument, fixture, API-observation, or frontend-action changes.
- Added offline timeout wiring validation without sleeps, startup/wait/cleanup fault injection, a different-status main-plus-cleanup failure case, progress assertions, and a mocked fill echo to guard against leaking fixture values. Independent read-only review approved the production/test changes and follow-up hardening.
- Parent next: rerun shell/JS checks and the task-only cutover. The last begin/end pair should identify the stalled boundary; inspect launch prerequisites (installed browser/runtime access, task-owned writable runtime/cache/temp paths, and container sandbox/capability compatibility) against that evidence rather than changing security flags speculatively. No workloads, NAS/SSH execution, or commits performed here; see the subsequent actual trace below.

## Bounded settings failure diagnostics follow-up

- In progress: finish the interrupted turn's settings-only diagnostic projection and regressions; keep all existing acceptance predicates and deadlines unchanged. Parent reports anonymous/authenticated startup now passes, but the first settings click exits 0 and `settings-leading-observed` returns literal false. Runtime log `/srv/workspaces/rustodon-audit-green/logs/browser-init-green.log` is parent-supplied evidence, not inspected here.
- Re-read the exact parent-supplied pinned `actions/settings.js` at the path above: 2000 ms leading/trailing debounce; `saved`/`me` guard inside callback; PUT `{ data }` excludes `saved`; successful promise dispatches `SETTING_SAVE`. The 2100 ms settling allowance does not prove idle debounce: guarded no-PUT invocations can still arm it. No deadline relaxation is justified by the supplied trace.
- Scope remains the two owned scripts and this issue. Tests-first additions are prepared but red/green execution is explicitly deferred: parent owns all tests/workloads, NAS/SSH, and commits.
- Ready for parent execution: settings failures now use a bounded diagnostic eval reporting only numeric/boolean/null values: checkbox counts/states, click timing, request counts/age/status/completion timing, expected Home-field matches, `saved` presence, and capped Resource Timing summaries. Keep the first eight requests so overflow cannot hide the leading candidate. Earlier resource counts distinguish pre-arming, settling, and pre-click activity; completed resources cannot reveal a no-PUT debounce invocation or establish that the debounce is idle.
- Tests cover no PUT, pending/late completion, wrong body and `saved`, scalar-only privacy projections, bounded lists retaining the first request, earlier-resource count distinctions, and diagnostic-error fallback preserving the original failure/cleanup. Independent read-only review approved the full settings-specific implementation and tests with no blocking findings. These new tests remain unexecuted; no local/NAS workloads, SSH, or commits performed.
- Handoff: parent can copy the full current owned script/test from `/Users/lainsoykaf/repos/ilar-task-remaining-browser/` into the matching parent paths. All initial/leading/trailing acceptance and timing bounds are preserved. Parent reports verified init-script integration and both startup endpoint fixes adopted/committed; all actual startup audits pass. The next diagnostic run must identify the settings failure before proposing behavioral changes. Issue remains open.

## First predicate diagnostics follow-up

- Parent's actual trace reports `session-id`, `network`, `open`, `wait-load`, `wait-predicate`, first `eval`, and cleanup all exit **0**; the gate then reports `api-observer-ready` failure. Chromium launch succeeded. This trace does not support a capability/sandbox cause or identify the eval result representation.
- Source analysis: the first gate can return false before probing (unsupported `responseStatus`, possibly full Resource Timing buffer, or rejected startup API response), on a probe status mismatch, or if either expected timing entry is absent. Separately, any CLI stdout other than literal `true` fails the shell contract even when the JS predicate returns true.
- Added safe result classification (`boolean-false`, `empty`, `quoted-boolean`, `object-like`, `other`) without dumping raw failed eval output or accepting an unverified envelope. A failed readiness gate additionally reports a fixed probe stage, up to two numeric probe statuses, timing capability/buffer flags, resource/API/cross-origin counts, and a capped numeric status histogram. No resource URLs, response bodies, headers, cookies, or bootstrap data are returned by that diagnostic JS. CLI formatting itself remains unverified.
- Tests cover object-like output rejection, classification, stop-at-gate behavior, diagnostic URL/query omission and value sanitization, and observed/missing timing stage bookkeeping. Independent read-only review found no blocking issues. These changes are prepared for parent NAS testing, not locally executed.
- Parent next: rerun the isolated gate and capture its safe classification/summary. On the same verified CLI, compare known-value `eval 'true'`, `eval 'false'`, and `eval 'Promise.resolve(true)'` outputs using the existing session/args invocation. Those synthetic results can establish formatting without inspecting any page/session/private state. Do not change the API allowlist or relax literal-boolean acceptance until the result representation and actual failed branch are known. Issue remains open; no workloads, NAS/SSH execution, or commits performed here.

## Current parent checkpoint

Real anonymous/authenticated startup audits now pass after the separately
committed extended-description and batch-account endpoints. Verified CLI init
scripts install HTTP tracking before navigation. The latest actual run reaches
Home settings but fails `settings-leading-observed`; a focused sanitized diagnostic
is prepared, with all timing and acceptance predicates unchanged. This is the
remaining browser blocker—not unexecuted browser infrastructure or a startup404.

## Actual browser acceptance (NAS)

The HTTP diagnostic conclusively identified CSRF422, despite valid payload and
15ms completion. The separate HTTPS-fixture issue fixes transport, not production
cookie policy or debounce timing. `browser-https-first.log` now records **browser
web-client smoke passed**, including actual leading/trailing frontend settings
PUTs, omitted `saved`, bootstrap plus rendered checkbox persistence after reload,
startup/navigation API audits, server-authenticated profile/settings/logout and
page-error checks. All existing deadlines are retained. The enclosing cutover
later failed Mastodon Puma startup; browser acceptance and full-cutover outcome
are recorded separately. Offline extracted-JS/shell and TLS tests pass on NAS;
independent merged integration review approved with no blocking findings.

Final acceptance: `final19-browser.log` passes browser actions **and** full cutover
rollback, after exact fixture-user settings snapshot/restoration was added. The
intermediate final18 run observed a still-pending PUT at the unchanged leading
response deadline; it is retained as a failed run, not relabeled green. Final19
passes with identical browser predicates, TLS policy, and timing bounds.
