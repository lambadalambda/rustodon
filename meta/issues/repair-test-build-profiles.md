# Repair default and release test build profiles

## Summary

An ignored SMTP worker test calls a feature-only helper without a gate; positive synthetic transport tests conflict with debug-only test APIs in release builds.

## Requirements

- Gate the individual feature-dependent test, not the whole worker target.
- Match positive transport tests to debug-only capabilities; retain production/release negative-capability tests.
- Do not enable production network bypasses or silently erase unrelated tests.

## Acceptance Criteria

- Reproduce default-feature compilation failure and affected release-profile failure before fixes.
- Default and feature-enabled debug gates pass; appropriate release profile tests/checks pass and capability denial remains asserted.

## Notes

- Source: [test coverage audit](../test-coverage-audit.md).
- User authorized this implementation plan on 2026-09-12; no live deployment is implied.
