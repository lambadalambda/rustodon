# Expand automated integration and harness gates

## Summary

Current CI omits focused HTTP regressions, startup and important security differential cases; expensive browser/cutover/peer gates and shell/Python harness tests need explicit lanes.

## Requirements

- Build on repaired profiles, permanent selectors and pinned fixture provisioning.
- Keep bounded fast/required and broader scheduled/manual lanes explicit; do not label ordinary checks as all tests.
- Update host instructions for the authorized NAS while preserving resource/source/secret isolation.

## Acceptance Criteria

- Checked-in CI/tasks select focused HTTP, startup and relevant security regression suites plus fast harness checks.
- Broader differential/browser/cutover/peer commands are discoverable and supported, with execution versus configured-only status recorded honestly.

## Notes

- Source: [test coverage audit](../test-coverage-audit.md).
- User authorized this implementation plan on 2026-09-12; no live deployment is implied.
