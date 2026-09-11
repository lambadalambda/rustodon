# Return empty results for unimplemented frontend API reads

## Summary

The user requests empty results instead of the current API 404 responses that repeatedly surface errors in the frontend.

## Requirements

- Inventory unimplemented API read endpoints and their expected frontend response shapes before changing behavior.
- Return endpoint-appropriate empty representations for supported fallback reads, preserving authentication/scope checks.
- Do not hide genuine missing-resource errors or report success for unsupported mutation operations.
- Keep non-API routes and federation protocol behavior unchanged.

## Acceptance Criteria

- Representative frontend polling/list requests no longer return implementation-placeholder 404 errors.
- Tests cover empty response shapes, applicable authorization and retained real-resource/mutation 404 behavior.
- Applicable checks pass on Secunda; live verification follows deployment.

## Notes

- Added during thread/profile-media repairs; keep this change and commit separately scoped.
