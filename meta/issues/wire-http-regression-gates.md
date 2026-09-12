# Wire permanent HTTP regression fixture gates

## Summary

Account search is omitted from the default fixture aggregate; empty-read and web-settings regressions require ad-hoc harness copies.

## Requirements

- Add named selectors for all three HTTP regressions and include them in the appropriate aggregate.
- Detect missing/renamed or empty selected tests before reporting success; preserve fixture setup/cleanup and least-privilege roles.
- Replace documented temporary-sed recipes with permanent commands.

## Acceptance Criteria

- Harness regressions reproduce omission/zero-selection risks before changes.
- Permanent commands execute account search, empty reads and web settings with nonzero tests; unknown/missing selections fail closed.

## Notes

- Source: [test coverage audit](../test-coverage-audit.md).
- User authorized this implementation plan on 2026-09-12; no live deployment is implied.

## Completion evidence (2026-09-12)

- Offline regression first failed on the omitted aggregate targets, then passed
  dispatch, missing-selection, listing-error and execution-error checks.
- Permanent NAS commands `schema-read-test v2_account_search`,
  `schema-read-test api_empty_reads`, and `schema-read-test web_settings` each
  passed one real fixture-backed HTTP test.
- Logs: `/srv/workspaces/rustodon-audit-main/logs/selectors-*.log`.
- Independent static review found no blockers. Exact-mode mock and environment
  propagation assertions remain optional coverage, not runtime evidence.
- Default aggregate includes all nine existing selectors; exact differential
  selections also reject absent tests before provisioning.
