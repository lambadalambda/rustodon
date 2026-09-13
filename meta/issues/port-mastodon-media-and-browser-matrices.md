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
- [Port mixed profile-media preservation matrix](port-mixed-profile-media-matrix.md)
- [Exercise delayed browser settings save and reload](exercise-browser-delayed-settings-save.md)
- [Port failed parent-fetch recovery and distribution matrix](port-parent-fetch-recovery-matrix.md)

## Completion evidence

All four selected subissues are satisfied and archived: media-state HTTP,
mixed-profile media, parent-fetch recovery/distribution, and actual browser
settings-save/reload. Each retained pinned expectations, meaningful rejection or
durable-state assertions, and permanent gate wiring. Exact topical evidence is
in the linked subissues; `final19-browser.log` also passes the enclosing full
cutover/rollback. Broader frontend/federation coverage is not inferred.
