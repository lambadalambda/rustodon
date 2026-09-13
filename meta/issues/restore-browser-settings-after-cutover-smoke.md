# Restore browser settings after cutover smoke

## Summary

The real browser now intentionally persists Home settings. The final cutover public-data comparison correctly detects the new web_settings row and sequence changes. Preserve and restore only the fixture user settings row and exact sequence state after verifying persistence; retain the full public-data equality check. Test both absent and existing settings; do not exclude the table or broadly delete state.

## Acceptance Criteria

- Source-backed focused regression, independently reviewed minimal correction.
- NAS-only execution; final affected gate passes without weakening production policy.

## Implementation evidence

`final17-browser.log` is the real PostgreSQL RED: browser smoke passes, but the
full public-data comparison detects exactly the intended fixture-user settings
row plus sequence changes. Snapshot/replay now preserves all user101 settings
columns (or row absence) and sequence last_value/is_called through the existing
private rollback file. Other users and the full public-data equality guard are
untouched. PostgreSQL14-compatible SQL uses one explicitly ordered result set.

NAS `browser-settings-rollback-red.log`:6 tests/8 failures before implementation.
`final18-rollback-offline.log` and the complete harness pass after implementation,
including preexisting/absent/NULL/quoted settings, unrelated-user preservation,
sequence failures and owned-file cleanup. Independent review approved.
At this intermediate checkpoint the real browser-plus-rollback gate was pending:
final18 stopped because its
leading PUT was still pending at the unchanged deadline, not a CSRF422.

Final acceptance: `final19-browser.log` passes the actual HTTPS browser **and**
full cutover/Rails reopen/public catalog-schema-data-auth/media equality checks.
`final19-cutover.log` independently passes without browser enabled. No rollback
assertion was removed, and no timing deadline was widened to obtain browser green.
