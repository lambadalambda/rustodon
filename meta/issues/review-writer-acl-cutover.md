# Review writer ACL and cutover safety

## Summary

Finish the read-only review of the writer privilege contract, operational-schema
ACL validation, and the documented cutover ordering.

## Requirements

- Verify that every privilege required by the configured writer is documented and
  that no unnecessary operational privilege is required.
- Verify preflight behavior before the operational schema and refresh function
  exist, as documented by the cutover runbook.
- Exercise public ACLs, grant options, default privileges, role attributes, and
  operational-role discovery against PostgreSQL.
- Preserve the pinned Mastodon checkout at
  `/workspace/rustodon/target/mastodon-v4.6.5` and do not change application
  files during the review.

## Acceptance Criteria

- The final review reports severity-ordered confirmed findings, residual risks,
  test coverage, and a release recommendation.
- Any confirmed documentation or validation gap is linked to an actionable
  remediation issue.

## Notes

- Existing operational-schema, preflight, and startup integration gates have
  passed; this issue tracks the remaining review questions rather than a known
  failing test.

## Findings

### Medium: Writer ACL provisioning is incomplete in the cutover runbook (resolved)

`docs/cutover.md:45-95` provisions the refresh functions and the runtime role's
operational grants, but does not provide the writer's required Mastodon table,
column, sequence, function, and operational-table grants. It also does not
provide the `PUBLIC` database/schema/function ACL cleanup required by
`WRITER_PRIVILEGE_QUERY` (`src/preflight.rs:315-1294`). A normal writer-enabled
deployment following the runbook literally therefore fails
`PF_WRITE_DATABASE_PRIVILEGES` before web or worker readiness.

The fixture setup supplied these missing prerequisites separately in
`tools/mastodon-fixture:1331-1386,1723-1905`, which is why the guarded tests pass.
This is resolved by `docs/mastodon-writer-grants.sql`, which is now invoked by
the fixture's writer setup and referenced by the cutover runbook.

## Findings Not Confirmed

- The pre-migration preflight ordering is intentional. Missing `rustodon` is
  accepted by `operational_schema_diagnostics`, and the preflight integration
  exercises that path.
- The apparent `PUBLIC EXECUTE` exemption for
  `rustodon_refresh_instances()` is covered by the earlier broad public-function
  ACL rejection; the guarded startup mutation rejects it.
- No additional writer privilege, migration, lock-order, pool-sizing, or pinned
  Mastodon compatibility finding was confirmed.

## Coverage

- `mise run check`: 193 passed, 2 ignored; formatting, Clippy, and dependency
  audit passed.
- `mise run operational-schema-integration`: passed.
- `mise run preflight-integration`: passed.
- `mise run startup-integration`: 4/4 passed.
- `mise run worker-integration`: 34/34 passed.
- `mise run mastodon-schema-integration`: 35/35 passed.
- `mise run fixture-restore-verify` and `mise run fixture-repro`: passed.
- `mise run differential`: 17 general cases plus notification and status
  authorization phases passed, 19/19 total.
- `git diff --check`: passed.

## Residual Risk And Recommendation

- Crash-time filesystem/database compensation, actual lease-fence cancellation,
  ambiguous commits, database-wide connection-budget stress, live SMTP, and live
  peer federation remain unproved.
- Hold a writer-enabled cutover release until the ACL recipe is rehearsed in a
  live cutover. The reviewed code and automated gates are otherwise green, but
  this is not approval for the complete v1 surface.

## Resolution

- The read-only review acceptance criteria are complete and the confirmed
  runbook gap is resolved by the executable ACL recipe. Live cutover and
  rollback rehearsal remain open; no application or pinned Mastodon files were
  changed by the review.
