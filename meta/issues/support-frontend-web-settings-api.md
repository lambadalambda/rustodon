# Support frontend web settings API

## Summary

The user still sees frontend 404 responses from `/api/web/settings` after the collection-read repairs. This previously deferred endpoint needs its real frontend contract, not an empty-success write stub.

## Requirements

- Inspect the pinned frontend/server contract and existing session, CSRF, settings persistence and privilege boundaries.
- Implement only the required methods with correct per-user persistence and authentication; never silently discard writes.
- Preserve existing settings, identity, database role isolation and genuine missing-resource errors.
- Run regression tests red to green on the user-authorized NAS worker; independently review before committing and deploying.

## Acceptance Criteria

- Regression coverage proves frontend-compatible settings updates/readback and authentication/CSRF/isolation/error behavior as applicable.
- Formatting, tests and strict Clippy pass; record any required grant delta separately.
- Deploy app-only with backup and verify the reported endpoint without exposing credentials or modifying unrelated user preferences.

## Notes

- Reported URL: https://rustodon-lain.tunnel.eosrift.com/api/web/settings
- Previous collection-only scope deliberately deferred API/web/settings writes; this is a new bounded implementation issue.
