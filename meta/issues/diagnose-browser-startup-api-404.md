# Diagnose unexpected browser startup API 404

## Summary

The real anonymous cutover/browser smoke fails its startup API audit before deliberate probes: four same-origin API resources, three HTTP200 and one HTTP404. Source suggests the about page extended_description request, but the precise failed endpoint is not yet runtime-confirmed.

## Requirements

- Identify the failed route using redacted path diagnostics. If the endpoint is missing, verify the pinned contract and add focused red/green coverage before implementing it. Do not allowlist the unexpected failure or weaken the browser audit.
- NAS-only workloads, independently reviewed topical changes, no live deployment.

## Acceptance Criteria

- Focused regressions and the real blocked gate pass; remaining blockers are explicit.

## Evidence

- Discovered by the combined remaining-audit gates on 2026-09-12.
- NAS logs: `browser-cutover-diagnostics.log`.


## Confirmed route and implementation scope

`browser-cutover-routes.log` identifies exactly
`/api/v1/instance/extended_description`:404 during anonymous About-page startup.
The real audit stays fail-closed. Pinned cached-image controller/model/serializer/
spec extracts establish blank/default response, timestamp and Markdown behavior;
base-controller extraction confirms the inherited limited-mode authentication.
Read-only canonical checkouts were not modified or replaced.

Tests cover inventory, normal GET/HEAD/slash/cache/CORS/OPTIONS, configured string
settings and limited-mode identity. Parent approved a narrow Rust Markdown
renderer dependency with a verified compatibility corpus, not a claim of complete
Redcarpet dialect parity. Tests-only NAS RED and pinned-engine corpus checks are
being run before the production overlay and NAS-only lockfile resolution.

## Endpoint implementation verified

- NAS serializer units5/5, route inventory1/1 and normal/limited HTTP cases pass.
- Exact pinned Redcarpet corpus11/11 passed; lock resolution added only
  pulldown-cmark0.13.4 and pulldown-cmark-escape0.11.0.
- Strict all-target/all-feature Clippy passes (`extended-lint-final.log`) after
  explicit imports and focused test lint corrections. Inventory118 assertion passes.
- Real anonymous startup API audit now passes. Authenticated startup reveals a
  separate missing batch-account route, tracked in
  [batch reads](serve-batch-account-reads.md); no404 allowlist was added.
- Security/correctness/DRY and final integrated reviews found no blockers. Full
  browser/cutover acceptance remains pending, so this issue stays open for now.
