# Fence browser reauthentication and recovery

## Summary

Prevent stale credential checks from authorizing writes after recovery, and rate-limit sensitive password challenges.

## Requirements

- Couple credential verification to session issuance and password changes using atomic checks or a validated credential generation.
- Use shared account/IP reauthentication limits before bcrypt across sensitive settings routes.
- Keep transaction fencing and abuse limits in separate topical commits.

## Acceptance Criteria

- Barrier-controlled login/reset and password-change/reset races cannot create authority based on the recovered password.
- Incorrect-password attempts across password and 2FA settings share a bounded budget; legitimate reauthentication works.

## Notes

- Findings R03, R13 in [the essential feature parity review](../essential-feature-parity-review.md), baseline `1861d33`.
- Review-only discovery: no implementation fix is included in the review commit. End-to-end scenarios remain to be executed unless the report explicitly records a parser reproduction.
