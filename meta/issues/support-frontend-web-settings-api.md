# Support frontend web settings API

## Summary

The user still sees frontend 404 responses from `/api/web/settings` after the collection-read repairs. This previously deferred endpoint needs its real frontend contract, not an empty-success write stub.

## Requirements

- Inspect the pinned frontend/server contract and existing session, CSRF, settings persistence and privilege boundaries.
- Implement only the required methods with correct per-user persistence and authentication; never silently discard writes.
- Preserve existing settings, identity, database role isolation and genuine missing-resource errors.
- Run regression tests red to green on the user-authorized NAS worker; independently review before committing and deploying.

## Acceptance Criteria

- Regression coverage proves frontend-compatible settings updates/readback and authentication/CSRF/isolation/error behavior as applicable.
- Formatting, tests and strict Clippy pass; record any required grant delta separately.
- Deploy app-only with backup and verify the reported endpoint without exposing credentials or modifying unrelated user preferences.

## Notes

- Reported URL: https://rustodon-lain.tunnel.eosrift.com/api/web/settings
- Previous collection-only scope deliberately deferred API/web/settings writes; this is a new bounded implementation issue.

## Implementation and pinned contract (NAS validation)

Implementation and independent review complete; **leave open pending ARM64 build,
deployment, and live verification**. No commit, live access, deployment,
instance environment, credential, or backup work was performed in this task.

Reference inspected read-only in the existing exact image
`ghcr.io/mastodon/mastodon@sha256:696439e1ada71d0cf3d51d4d6a4744d6e40b57aafa64980b18f4d3b78230d0cf`
(`/opt/mastodon`, pinned revision `1440d55b139e39ec722c2a3db7f60b66cd889048`).
No replacement checkout was fetched. Reference excerpts are under the NAS task's
`ops/reference/{contract,details,spec,bootstrap}.txt`:

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
**Parent operator approved this necessary bounded delta after independent review;
it has not yet been applied live.**

## Reproducible NAS evidence

User-authorized rootful worker: `192.168.1.186`; all connections sequential using
`ssh -o ConnectTimeout=10 -o HostKeyAlias=podman-worker.local root@192.168.1.186`.
No Mac/Secunda builds, tests, formatting, lint, or containers were used.

Task workspace: `/srv/workspaces/rustodon-web-settings-c81a` (`W` below).
Only Git-tracked regular files (including intent-to-add task files) were rsynced
with an explicit files list, without `--delete`; the three dangling public links
were recreated separately from their tracked targets. No `.git`, `.env`,
`.local-instance`, backups, or unrelated untracked files were synced. Task source
was chowned `0:0` after sync for mode-600 files. Compiler output was copied from
`/srv/workspaces/rustodon-follow-repairs-b2937cf/source/target` into this task only;
no canonical reference or other workspace was modified.

Tool image: `localhost/rustodon-nas-tools:b2937cf`, verified ID
`14233b9e6d8403d8d029c36f6dfd495a424b879cf44775239fedeed419566caf`.
All validation uses this common invocation (exact scripts retained in
`$W/ops/{validate,final}.sh`; commands also in `$W/logs/*.command`):

```sh
podman run --rm --name rustodon-web-settings-<gate> --pull=never \
  --cap-drop=ALL --network host \
  --cpu-period=100000 --cpu-quota=400000 --memory=8g --memory-swap=8g \
  -e CARGO_BUILD_JOBS=3 \
  -v /run/podman/podman.sock:/run/podman/podman.sock \
  -v "$W/source:$W/source" -w "$W/source" \
  localhost/rustodon-nas-tools:b2937cf <command>
```

The image's remote Podman wrapper retains the harness's sanitized environment,
host-network fixture access and same-path fixture binds. The temporary executable
`source/tools/mastodon-fixture-web-settings` differs from the authoritative harness
only by replacing `--lib web::account_search_tests` with
`--lib web::web_settings_tests`; setup, middleware, grants and cleanup are intact.
It is not a tracked harness fork.

| Gate / exact command inside tool container | Result | Log under `$W/logs/` |
| --- | --- | --- |
| `tools/mastodon-fixture-web-settings schema-read-test v2_account_search` **before production edits** | RED: one real HTTP regression failed, expected 200, got 404 `{"error":"Not Found"}` | `red.log` |
| Same command, final source | GREEN: 1 comprehensive HTTP/persistence test passed (fresh pool and actual HTML reload/bootstrap; bootstrap CSRF pair; session-only form PATCH; bearer precedence/no scopes; app/anonymous/invalid/expired/revoked/user lifecycle; stale session; shape/malformed/bounded/chunked bodies; user/default isolation; concurrent first saves; read/write ACL failures; failed DB write returns 500; genuine missing methods; real empty-snapshot clearing) | `green.log` |
| `cargo fmt --all -- --check` (after NAS `cargo fmt --all`) | Passed | `fmt.log` (empty on success) |
| `cargo test --locked --all-targets --all-features` | Passed: 29 suites, 431 passed, 0 failed, 163 ignored; includes route inventory and ordinary filesystem-permission preflight with **all root capabilities dropped** | `ordinary.log` |
| `cargo clippy --locked --all-targets --all-features -- -D warnings` | Passed | `clippy.log` |
| `cargo check --locked --lib --bin rustodon` | Passed, default-feature production targets | `production-check.log` |
| `tools/mastodon-fixture startup-test` | Passed: 5 tests, 0 failed, 1 filtered; all new grant mutations plus existing startup/browser/worker fencing, 593.35 seconds | `startup.log` |

The first long validation SSH timed out while startup continued; a subsequent
**sequential** `podman wait rustodon-web-settings-startup` returned 0 and the final
log confirmed harness success/cleanup. No task containers remain. Intermediate
iterations caught and fixed the fixture's later sequence REVOKE overriding an
initially early grant, and updated the existing explicit route/catalog counts.
`green-initial.log` is an intermediate diagnostic, not final green evidence.

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

## Independent review and operator approval

- Read-only reviewer `d1831fd8-7caf-4afc-999c-4e842c811cd7` found no blocking
  correctness/security/architecture/DRY finding in the identified repair paths,
  and approved the exact baseline-guarded apply/inverse SQL pair. The reviewer
  did not independently rerun tests or certify a Git-diff inventory.
- Parent read-only live metadata confirmed `rustodon_runtime` already has table
  SELECT, while `rustodon_writer` lacks the new table/column/sequence privileges.
  There were zero existing `web_settings` rows at inspection. No preferences or
  credentials were read. The requested repair authorizes this narrowly necessary
  writer delta; no additional schema, runtime, or emoji privilege changes are needed.
- Deployment-only scripts: `.local-instance/apply-web-settings-writer-grants.sql`
  and `revert-web-settings-writer-grants.sql`, used with the unchanged reviewed
  transaction/uncertain-apply-safe app replacement helper.

## Deployment checkpoint — SSH signing blocked

- Reviewed commits: `292b874` (bounded grants/fences/fixture) and `a29df56`
  (settings endpoint/persistence/bootstrap). Working source was checksum-matched
  to the NAS workspace, including all three public symlink targets.
- NAS ARM64 production build passed (`--locked --release --no-default-features`,
  explicit Rust 1.97.1). Candidate image:
  `859b97ecc2cbd42c47553a8760dd698d7cf57f2c8d2ce3f75bb5240cbf068777`,
  Linux/ARM64, revision `a29df561bc28c6f90b1f1518e969b5ef94222511`.
  NAS loader and CLI checks passed. OCI archive at
  `/srv/workspaces/rustodon-web-settings-c81a/rustodon-a29df56-arm64.oci`, SHA-256
  `8d24ae0482115a4babe56685aef82246db56dfd11423b314be2055130600d38a`.
- Transfer to the local deployment host failed because the SSH agent refused
  signing. One sequential retry failed with agent communication error; no further
  attempts or credential workaround. Asked the user to unlock the SSH agent.
- **Not deployed.** No new live grants, backup/cutover or settings mutation occurred.
  Existing apps remain on `9ffaa0db...` / source `b2937cf`; a safe unauthenticated
  PUT before cutover still returned the reported 404. Issue stays open.
- Resume with archive transfer/checksum verification, native local loader/CLI and
  all runtime-input hashes, then reviewed grant-aware app-only backup/cutover.
  `.local-instance/verify-web-settings-routing.py` is prepared to verify routing,
  CSRF and authentication rejection without login or settings writes; successful
  signed-in save/reload is fixture-proven but still needs live user confirmation.

## Deployed after SSH unlock — 2026-09-12 UTC

- User unlocked the SSH agent. Archive transfer succeeded with the recorded
  SHA-256; Linux/ARM64 image identity and full `a29df56` source label matched.
  Native local loader/CLI smoke checks and all **6062** runtime-input hashes
  passed, including bundled frontend assets and the CA certificate bundle.
- Reviewed app-only helper completed at `20260912T063346Z`: old preflight,
  consistent backup, both apps stopped, exact web-settings grant transaction,
  new preflight, app replacement and identity/readiness checks. Both apps now use
  `859b97ecc2cbd42c47553a8760dd698d7cf57f2c8d2ce3f75bb5240cbf068777`.
  PostgreSQL/Redis containers, persistent volumes, accounts and origin were preserved.
- Backup: `.local-instance-backups/20260912T063351Z/` (not restore-tested).
  Evidence: `.local-instance/logs/deploy-20260912T063346Z/`.
  Prior apps are stopped with suffix `-rollback-20260912T063346Z`, image `9ffaa0db...`.
  Rollback to that pair requires reversing **only the new web-settings ACL delta**
  before restart; retain existing settings rows and earlier emoji/runtime grants.
- Live public PUT without CSRF now returns the expected 422 CSRF error, not 404.
  PUT and PATCH with a valid anonymous CSRF pair return the expected 422
  authenticated-user requirement. Cookies stayed in memory and were not printed.
  `web-settings-routing.jsonl` records these negative probes; no live preferences
  were mutated. The table still had zero settings rows at this check.
- Local/public/worker readiness passed, zero queued jobs. Two **pre-existing**
  media dead letters (799 and 800, failed 2026-09-11 16:44 UTC) remain untouched:
  profile image invalid/processing bounds and unsupported remote-media response
  content type. They predate this deployment and are not settings-save failures.
- Authenticated save/reload is proven by the NAS HTTP/persistence fixture, not
  by these unauthenticated live probes. **Keep issue open pending the user's
  normal signed-in frontend retry**; no access token was obtained or session
  created to manufacture positive live evidence. Direct GET remains unsupported
  by the pinned contract; the frontend uses PUT (PATCH is also supported).

## Permanent regression command

Use `tools/mastodon-fixture schema-read-test web_settings` on the authorized
isolated Linux worker. It replaces the historical temporary harness below/above;
the original execution evidence remains unchanged.
