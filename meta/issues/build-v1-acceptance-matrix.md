# Build the executable v1 acceptance matrix

## Summary

Map every authoritative v1 requirement to implementation and proof.

## Requirements

- Link every `docs/v1-scope.md` bullet and invariant to an issue, endpoint or
  command, and automated or explicit manual acceptance case.
- Cover every supported route/command and fail on unproved or falsely
  advertised capabilities.

## Acceptance Criteria

- The pinned frontend and mobile client complete normal v1 use with no unmapped
  in-scope requirement.

## Progress

- Added [`docs/v1-acceptance-matrix.md`](../../docs/v1-acceptance-matrix.md),
  tracing the v1 scope bullets, non-obvious invariants, executable gates, and
  top-level acceptance criteria to implementation and proof status.
- Added `V1_REQUIRED_API_ROUTES` beside `API_ROUTE_INVENTORY`. A unit test now
  fails if a required v1 method/path is absent or falsely marked unsupported;
  the existing inventory test still checks every advertised route for unique
  protocol contracts.
- The complete 20-phase Rails-versus-Rust differential suite passes against the
  pinned Mastodon 4.6.5 fixture, including browser authentication, the web
  client shell, REST contracts, media, reports, federation discovery, OAuth,
  writes, notification writes, and status authorization.
- The fixture now seeds representative Mastodon-owned preservation rows for
  migrations, announcements, appeals, backups, imports, report notes, and Web
  Push subscriptions. SQL/Rails restore verification plus schema, differential,
  and cutover snapshots prove their values and foreign-key links survive the
  Rustodon process and Mastodon reopen.
- The issue remains open for the recorded browser/mobile flows, cross-surface
  policy matrix, pinned live Mastodon peer, and production cutover/rollback.
- The matrix now records the signed POST DNS/redirect/timeout/response-limit
  fixtures and the restored relationship duplicate-convergence coverage; both
  are locally green while peer-side idempotency remains external evidence.
- Current acceptance gates also pass for the 35-case Mastodon schema suite,
  operational schema and streaming integration, startup safety, preflight,
  worker integration (45/45), and the cutover/rollback rehearsal.

## Closed 2026-09-23

Implementation complete. Remaining peer, browser, mobile and production
evidence moved to [complete-v1-external-acceptance](complete-v1-external-acceptance.md).
