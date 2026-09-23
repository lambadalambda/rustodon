# Support frontend web settings API

## Summary

The user still sees frontend 404 responses from `/api/web/settings` after the collection-read repairs. This previously deferred endpoint needs its real frontend contract, not an empty-success write stub.

## Requirements

- Inspect the pinned frontend/server contract and existing session, CSRF, settings persistence and privilege boundaries.
- Implement only the required methods with correct per-user persistence and authentication; never silently discard writes.
- Preserve existing settings, identity, database role isolation and genuine missing-resource errors.
- Run regression tests red to green on the isolated worker; independently review before committing and deploying.

## Acceptance Criteria

- Regression coverage proves frontend-compatible settings updates/readback and authentication/CSRF/isolation/error behavior as applicable.
- Formatting, tests and strict Clippy pass; record any required grant delta separately.
- Deploy app-only with backup and verify the reported endpoint without exposing credentials or modifying unrelated user preferences.

## Notes

- Reported endpoint: `/api/web/settings` on the existing public origin.
- Previous collection-only scope deliberately deferred API/web/settings writes; this is a new bounded implementation issue.

## Implementation and pinned contract (isolated worker validation)

Implementation and independent review complete; **leave open pending ARM64 build,
deployment, and live verification**. No commit, live access, deployment,
instance environment, credential, or backup work was performed in this task.

Reference inspected read-only in the existing exact image
`ghcr.io/mastodon/mastodon@sha256:696439e1ada71d0cf3d51d4d6a4744d6e40b57aafa64980b18f4d3b78230d0cf`
(`/opt/mastodon`, pinned revision `1440d55b139e39ec722c2a3db7f60b66cd889048`).
No replacement checkout was fetched. Reference excerpts were retained in
historical external artifacts not in the repository:

- `app/javascript/mastodon/actions/settings.js` sends `PUT /api/web/settings`
  with `{data: <complete settings snapshot>}`, omitting its local `saved` flag.
  `api.ts` sends the bootstrap bearer and `X-CSRF-Token`.
- `config/routes/api.rb` declares a singleton update resource (PUT and PATCH).
  `api/web/settings_controller.rb` replaces `Web::Setting.data`, returning HTTP
  200 with `{}`. It has no OAuth scope requirement, but requires a functional user.
  `api/web/base_controller.rb` requires CSRF, including on bearer requests, with
  JSON 422 `Can't verify CSRF token authenticity.` on failure.
- `spec/requests/api/web/settings_spec.rb` also exercises session-authenticated
  PATCH/form data (values remain strings), and unauthenticated 422 behavior.
- `app/helpers/application_helper.rb#render_initial_state` restores the signed-in
  user's `Web::Setting.data`; the settings reducer hydrates this snapshot.
- Existing `web_settings` has a unique `user_id` index, JSON `data`, timestamps,
  and a cascading user foreign key. This is **not** `users.settings` (posting
  defaults/server preferences).

Changes:

- PUT/PATCH, including trailing-slash routes, are inventoried private writes.
  GET/POST/DELETE remain genuine missing routes, not fake read/write successes.
- Reuse signed browser CSRF cookies with the client's header, existing browser
  session lookup and bearer/user lifecycle validation. A supplied bearer takes
  precedence; invalid bearers never fall back to another cookie identity. No
  changes to OAuth scopes, browser login, or ordinary API CSRF policy.
- Persist atomic whole-snapshot replacements with a per-account transaction lock,
  existing account-write checks, user/account identity binding, and the unique
  user upsert. Concurrent first saves create one row; last writer wins as for the
  pinned full-snapshot contract. Posting defaults are not read-modify-written.
- Fresh repository reads feed the existing escaped frontend bootstrap serializer;
  absent/SQL-null settings yield `{}`. Unknown nested settings and JSON types
  survive, as do Rack form strings. `{data:{}}` really clears the snapshot.
- Deliberate input hardening: require an object `data` and reject uploads, missing,
  null, scalar, or array roots with JSON 422 rather than persisting frontend-breaking
  data (the Rails controller itself does not validate these shapes). Existing
  bounded middleware enforces 4 MiB and its read timeout before parameter expansion,
  for both declared-length and chunked bodies, plus existing JSON/form depth limits.
  Malformed JSON remains the existing framework JSON 400 shape. Database failures
  remain sanitized 500 responses, never successful discarded writes.

## Required privilege delta — NOT applied live

No schema migration is needed. The writer needs only:

```sql
GRANT SELECT ON TABLE public.web_settings TO :"writer_role";
GRANT INSERT (user_id, data, created_at, updated_at)
  ON TABLE public.web_settings TO :"writer_role";
GRANT UPDATE (data, updated_at)
  ON TABLE public.web_settings TO :"writer_role";
GRANT USAGE ON SEQUENCE public.web_settings_id_seq TO :"writer_role";
```

The reader needs SELECT on `public.web_settings` (already covered by the fixture's
read-only SELECT grants). Production grant documentation, fixture grants, critical
catalog inventory, required privileges and exact allowed ACL fences are updated.
No table-wide INSERT/UPDATE, DELETE, ownership/creation-identity rewrite,
sequence SELECT/UPDATE, grant option, schema rights, or function rights are added.
Startup tests reject missing individual privileges and these broader alternatives.
This bounded delta had not yet been applied live.

## Reproducible isolated-worker evidence

All execution used a bounded isolated worker and task-owned workspace. No local
or alternate-worker builds, tests, formatting, lint, or containers were used.

Only Git-tracked regular files, including intent-to-add task files, were
transferred. Git metadata, environment files, instance state, backups, unrelated
untracked files, and canonical references were excluded; no destination deletion
occurred. The three dangling public links were recreated from their tracked
targets. Compiler output was copied into this task only, and only task source
ownership was adjusted to read mode-600 inputs.

Validation used no image pulls, dropped all root capabilities, bounded CPU/memory,
and preserved the harness's sanitized environment and fixture access. A temporary
filtered selector ran the web-settings fixture without changing setup, middleware,
grants, or cleanup. It was not a tracked harness fork. Historical external run
artifacts are not in the repository.

| Gate | Result |
| --- | --- |
| Temporary filtered web-settings selector, **before production edits** | RED: one real HTTP regression failed, expected 200, got 404 `{"error":"Not Found"}` |
| Same command, final source | GREEN: 1 comprehensive HTTP/persistence test passed (fresh pool and actual HTML reload/bootstrap; bootstrap CSRF pair; session-only form PATCH; bearer precedence/no scopes; app/anonymous/invalid/expired/revoked/user lifecycle; stale session; shape/malformed/bounded/chunked bodies; user/default isolation; concurrent first saves; read/write ACL failures; failed DB write returns 500; genuine missing methods; real empty-snapshot clearing) |
| `cargo fmt --all -- --check` (after `cargo fmt --all`) | Passed; output was empty on success |
| `cargo test --locked --all-targets --all-features` | Passed: 29 suites, 431 passed, 0 failed, 163 ignored; includes route inventory and ordinary filesystem-permission preflight with **all root capabilities dropped** |
| `cargo clippy --locked --all-targets --all-features -- -D warnings` | Passed |
| `cargo check --locked --lib --bin rustodon` | Passed, default-feature production targets |
| `tools/mastodon-fixture startup-test` | Passed: 5 tests, 0 failed, 1 filtered; all new grant mutations plus existing startup/browser/worker fencing, 593.35 seconds |

The first long validation client wait timed out while startup continued; a
subsequent sequential wait returned 0 and confirmed harness success/cleanup. No
task containers remained. Intermediate iterations caught and fixed the fixture's
later sequence REVOKE overriding an initially early grant, and updated the
existing explicit route/catalog counts. An intermediate diagnostic run was not
treated as final green evidence.

Ordinary tests do not execute ignored pinned-source or worker integration tests;
**no claim that pinned-source contracts or the full ignored worker suite ran**.
The known default-feature all-target worker-test `with_complete_fault` compile
issue was not expanded or worked around; the default-feature production check
above passed. No new dependencies were added.

## Parent deployment / recovery handoff

- Independent review and commits are parent-owned; changes are intentionally
  uncommitted. Review/approve the ACL delta before ARM64 build and rollout.
- Before deployment, use the established backup/recovery process for the app,
  affected `web_settings` data, and prior role ACLs. No new schema/data migration,
  frontend rebuild, unrelated preference rewrite, or credential rotation is needed.
- After approved ACL changes, run preflight with the actual least-privilege roles,
  deploy the app, and verify a controlled user's normal frontend save and reload.
  Verify the signed-in PUT/PATCH contract, not an unauthenticated GET success.
  Preserve/restore that controlled user's original snapshot after a probe.
- Rollback preserves the existing Mastodon-compatible `web_settings` rows. Stop
  the new app and reverse only this new ACL delta before starting the prior app:
  its exact writer-privilege fence rejects the additional grants. Coordinate any
  row restore so concurrent legitimate user changes are not overwritten.

## Independent review and deployment boundary

- Independent read-only review found no blocking correctness, security,
  architecture, or DRY finding in the identified repair paths and approved the
  exact baseline-guarded apply/inverse SQL pair. It did not independently rerun
  tests or certify a Git-diff inventory.
- Parent read-only live metadata confirmed `rustodon_runtime` already has table
  SELECT, while `rustodon_writer` lacks the new table/column/sequence privileges.
  There were zero existing `web_settings` rows at inspection. No preferences or
  credentials were read. The bounded repair requires only this writer delta; no
  additional schema, runtime, or emoji privilege changes are needed.
- An independently reviewed external deployment process was prepared for the
  narrowly required ACL delta; unpublished helper details are not in the repository.

## Deployment checkpoint — not deployed

- Reviewed commits: `292b874` (bounded grants/fences/fixture) and `a29df56`
  (settings endpoint/persistence/bootstrap). Working source was checksum-matched,
  including all three public symlink targets.
- The external ARM64 production build passed
  (`--locked --release --no-default-features`, explicit Rust 1.97.1) at revision
  `a29df561bc28c6f90b1f1518e969b5ef94222511`. External loader and CLI checks
  passed; the build artifact is not in the repository.
- **The build artifact was not deployed at this checkpoint.** No new live grants,
  backup/cutover or settings mutation occurred. Existing apps remained on source
  `b2937cf`; a safe unauthenticated PUT before cutover still returned the reported
  404. The issue remained open.
- Resume through the reviewed external application-only deployment process.
  Signed-in save/reload was fixture-proven but still needed live user confirmation.

## Deployment completed — 2026-09-12 UTC

- Transfer later succeeded; the recorded Linux/ARM64 package identity and full
  `a29df56` source label matched. Native loader/CLI smoke checks passed, and all
  **6062** packaged runtime inputs passed hash verification, including bundled
  frontend assets and the CA certificate bundle.
- The reviewed application-only deployment completed on **2026-09-12**: old
  preflight, consistent backup, both apps stopped, exact web-settings grant
  transaction, new preflight, app replacement, and identity/readiness checks.
  PostgreSQL and Redis services, persistent volumes, accounts, and origin were
  preserved.
- The backup was not restore-tested; deployment evidence is a historical external
  artifact not in the repository. Prior apps remain stopped and available for
  rollback. Restoring them requires reversing **only the new web-settings ACL
  delta** before restart while retaining existing settings rows and earlier
  emoji/runtime grants.
- Public PUT without CSRF returned the expected 422 CSRF error, not 404. PUT and
  PATCH with a valid anonymous CSRF pair returned the expected 422 authenticated-
  user requirement. Cookies stayed in memory and were not printed. The historical
  external run recorded these negative probes; no live preferences were mutated,
  and the table still had zero settings rows at this check.
- Local/public/worker readiness passed with zero queued jobs. Two **pre-existing**
  media dead letters remained untouched: one for profile-image processing bounds
  and one for unsupported remote-media response content type. They predated this
  deployment and were not settings-save failures.
- Authenticated save/reload is proven by the isolated-worker HTTP/persistence
  fixture, not by these unauthenticated live probes. **Keep issue open pending the
  user's normal signed-in frontend retry**; no access token was obtained or session
  created to manufacture positive live evidence. Direct GET remains unsupported
  by the pinned contract; the frontend uses PUT (PATCH is also supported).

## Permanent regression command

Use `tools/mastodon-fixture schema-read-test web_settings` on an isolated Linux
worker. It replaces the historical temporary harness; the original execution
evidence remains unchanged.

## Closed 2026-09-23

Closed by the user on 2026-09-23. Deployed 2026-09-12; the fixture proves save/reload.
