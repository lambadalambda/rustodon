# Port selected Mastodon media and browser behavior matrices

## Summary

After foundational gates and the audited fixes, extend existing tests with independent upstream expectations rather than duplicate already-covered examples.

## Requirements

- Start with media processing/ownership/attachment-state, mixed profile media preservation, browser delayed settings action/save/reload, and failed parent-fetch recovery/distribution.
- Verify discovery cases from local 4.7-alpha against pinned 4.6.5 before adopting expectations.
- Keep each matrix a separate topical red/green change and explicitly scope unsupported formats/features.

## Acceptance Criteria

- New matrices assert expected responses, decoded media or durable side effects, and no mutation on rejected operations.
- Real browser actions detect unexpected API failures while preserving deliberate auth/missing-resource errors.
- Tests are permanently wired into appropriate gates rather than ad-hoc scripts.

## Notes

- Source: [test coverage audit](../test-coverage-audit.md).
- User authorized this implementation plan on 2026-09-12; no live deployment is implied.

## Subissues

- [Pinned media-state HTTP matrix](port-media-state-http-matrix.md)
