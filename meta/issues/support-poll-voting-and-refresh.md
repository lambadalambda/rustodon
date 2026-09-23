# Support polls in the bundled frontend

## Summary

The bundled composer exposes poll creation and historical or remote polls can render, but Rustodon does not create polls and has no poll show or vote routes. Posting a poll cannot work normally; clicking Vote on a visible poll fails, and the frontend cannot refresh results.

## Requirements

- Implement poll creation through `POST /api/v1/statuses` together with `GET /api/v1/polls/:id` and `POST /api/v1/polls/:id/votes`, returning Mastodon 4.6.5 Poll responses.
- Enforce option/count/expiry bounds, status visibility, poll expiry, account eligibility, choice bounds, single/multiple-choice rules, and duplicate-vote semantics.
- Update tallies and voter state transactionally and emit the frontend/federation effects required for ordinary remote and local poll creation and voting.

## Acceptance Criteria

- A signed-in user can create a bounded poll in the bundled composer, publish it, and see the poll on the resulting status.
- A signed-in user can vote on an eligible poll from the bundled frontend and immediately sees the returned selected choices and updated counts.
- Reloading or refreshing a poll returns current results.
- Expired, hidden, invalid, duplicate, and unauthorized votes match Mastodon 4.6.5 behavior.
- Focused differential and browser coverage exercise a visible poll through vote and refresh.

## Evidence

- Mastodon 4.6.5's bundled frontend calls `GET /api/v1/polls/:id` and `POST /api/v1/polls/:id/votes` with `choices: string[]`, then imports the returned Poll JSON.
- Rustodon serializes existing polls but registers neither read/vote route; both fall through to JSON 404. Its status-write contract explicitly excludes poll creation.
- This makes the bundled frontend's ordinary poll lifecycle unavailable end to end rather than merely omitting an optional search/API feature.

## Status 2026-09-23

Implemented (see git log and DEVLOG). Acceptance waits for the final-tree
sweep in [complete-v1-external-acceptance](complete-v1-external-acceptance.md).
