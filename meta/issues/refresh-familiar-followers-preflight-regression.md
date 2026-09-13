# Refresh familiar followers preflight classification

## Summary

The required REST protocol differential still classifies the explicitly supported familiar_followers fallback as unsupported. Existing route inventory and focused web tests require successful CORS preflight.

## Requirements

- Move familiar_followers into positive paired preflight coverage; retain a genuinely unknown route negative control. Preserve production routing/authentication/fallback behavior. Verify pinned runtime responses rather than assume them.
- NAS-only workloads, independently reviewed topical changes, no live deployment.

## Acceptance Criteria

- Focused regressions and the real blocked gate pass; remaining blockers are explicit.

## Evidence

- Discovered by the combined remaining-audit gates on 2026-09-12.
- NAS logs: `differential-required-remaining.log`.

## Implementation notes

- Existing required NAS `rest_protocol_contracts` RED reported: `unsupported route preflight was advertised: /api/v1/accounts/familiar_followers`.
- Extend the account-search paired status/body/full CORS-header comparison to familiar followers, including its explicitly registered trailing-slash route. Keep `/api/v1/unknown` as the Rust 404/no-advertised-methods negative control.
- Test-only correction; production routing, authentication, and fallback behavior are unchanged. Existing focused web tests already cover the route inventory and successful preflight; no internal API exposure is needed.
- No tests, builds, formatting, NAS/SSH workloads, or commits run in this implementation worktree, as requested. GREEN remains pending the parent's exact pinned 4.6.5 NAS gate; this issue remains open. No current-4.7 or extracted-source evidence is treated as the oracle.

Independent review of the exact incremental patch found no actionable findings.
Parent applied only the two preflight hunks; NAS GREEN and signing remain blocked
on SSH-agent unlock.

## Completion

The corrected pinned `rest_protocol_contracts` differential passes on NAS
(`rest-preflight-green.log`,117.56s). Strict all-target/all-feature Clippy also
passes. Independent review approved the two-hunk test-only correction. Both
familiar-followers forms now compare against real pinned Rails; the unknown-route
negative guard remains. No production behavior was changed.
