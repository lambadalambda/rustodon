# Restore browser settings after cutover smoke

## Summary

The real browser now intentionally persists Home settings. The final cutover public-data comparison correctly detects the new web_settings row and sequence changes. Preserve and restore only the fixture user settings row and exact sequence state after verifying persistence; retain the full public-data equality check. Test both absent and existing settings; do not exclude the table or broadly delete state.

## Acceptance Criteria

- Source-backed focused regression, independently reviewed minimal correction.
- Isolated-worker-only execution; final affected gate passes without weakening production policy.

## Implementation evidence

A historical external run recorded the real PostgreSQL RED: browser smoke passes, but the
full public-data comparison detects exactly the intended fixture-user settings
row plus sequence changes. Snapshot/replay preserves all fixture-user settings
columns (or row absence) and sequence `last_value`/`is_called` through the bounded
rollback state. Other users and the full public-data equality guard are untouched.
PostgreSQL 14-compatible SQL uses one explicitly ordered result set.

A historical external isolated-worker run recorded 6 tests / 8 failures before implementation.
The offline rollback tests and complete harness passed after implementation,
including preexisting/absent/NULL/quoted settings, unrelated-user preservation,
sequence failures and owned-file cleanup. Independent review approved.
At this intermediate checkpoint the real browser-plus-rollback gate was pending:
the run stopped because its leading PUT was still pending at the unchanged
deadline, not because of a CSRF 422.

Final acceptance: a historical external run passed the actual HTTPS browser **and**
full cutover/Rails reopen/public catalog-schema-data-auth/media equality checks.
A separate historical external run passed without browser enabled. No rollback
assertion was removed, and no timing deadline was widened to obtain browser green.
