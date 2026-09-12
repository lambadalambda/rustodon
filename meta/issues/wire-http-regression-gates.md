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
