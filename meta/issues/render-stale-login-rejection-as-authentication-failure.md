# Render stale login rejection as authentication failure

## Summary

A login whose verified password becomes stale during recovery is now safely denied when creating its session, but the browser handler maps that denial to HTTP 500. Return an intentional authentication failure without weakening the credential fence.

## Requirements

- Distinguish `WriteError::Unauthorized` from genuine database/internal errors after `create_browser_session`.
- Define an appropriate browser status/message or redirect using the existing sign-in contract and pinned Mastodon reference where applicable.
- Preserve credential-generation checks, shared limits, session/token revocation and generic user-facing errors; do not disclose recovery state or account existence.

## Acceptance Criteria

- A barrier-controlled login/recovery HTTP regression receives the chosen authentication-failure response rather than 500, with no newly issued session or live token.
- Fresh login still succeeds and genuine internal errors remain distinguishable internally.
- Focused authentication/recovery regressions and independent review pass on an isolated worker.

## Notes

- Follow-up to [reauthentication and recovery fencing](fence-browser-reauthentication-and-recovery.md), R03.
- The security fence passed remote regressions; the remaining HTTP rendering behavior was source-reviewed, not a new security bypass.

- Tracking only: no implementation or tests were performed for this issue. Builds, tests, formatting, lint and containers remain isolated-worker-only.
