# Close reviewed client workflow gaps

## Summary

Repair qualified local mentions, hashtag stream fan-out, and public OAuth/discovery contract inconsistencies.

## Requirements

- Normalize fully qualified local mention handles to local account identity.
- Align followed-hashtag user-stream lifecycle events with supported home timeline eligibility.
- Allow public OAuth clients to revoke their own tokens while retaining application ownership checks.
- Only advertise implemented OAuth response modes and reject unsupported requests explicitly.
- Implement each client workflow in a separate topical commit, not one broad rewrite.

## Acceptance Criteria

- Qualified and short local mentions grant identical direct-post access and notifications on create/edit.
- A hashtag-only follower receives eligible create/edit/delete events corresponding to REST home membership.
- Public PKCE authorize/exchange/revoke rejects the revoked bearer and cannot revoke another application token.
- Advertised response modes work on approval and denial; unsupported modes have a tested rejection contract.

## Notes

- Findings R11, R12, R14, R15 in [the essential feature parity review](../essential-feature-parity-review.md), baseline `1861d33`.
- Review-only discovery: no implementation fix is included in the review commit. End-to-end scenarios remain to be executed unless the report explicitly records a parser reproduction.
