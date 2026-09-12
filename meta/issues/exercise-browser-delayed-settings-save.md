# Exercise delayed browser settings save and reload

## Summary

Extend the existing browser gate beyond shell mounting with a real frontend setting action, debounced PUT, persisted state and reload. Preserve deliberate authentication/missing-resource errors while failing unexpected API responses.

## Acceptance Criteria

Use agent-browser on an isolated Linux fixture only. Confirm pinned 4.6.5 frontend action timing/contracts; demonstrate a failing behavioral baseline and browser green. Do not duplicate the already passing direct HTTP PATCH test or touch signed-in live sessions.

## Notes

- Subissue of [selected matrix ports](port-mastodon-media-and-browser-matrices.md).
- Tests first; separate topical implementation and independent review.
- Pending; no implementation or execution evidence claimed.

## Shared fixture prerequisite

Parent replaced the cutover/differential JPEG-only upstream-path dependency with
the existing exact vendored corpus and early verifier. NAS prerequisite red
detected the old path; green verified corpus and corruption/missing checks.
Logs: `rustodon-audit-green/logs/extended-media-prereq-{red,green}.log`.
Independent static review passed. Full browser execution remains pending; no
full-source oracle guard was weakened.
