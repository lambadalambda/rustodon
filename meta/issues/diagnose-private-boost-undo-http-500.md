# Diagnose private boost Undo returning HTTP 500

## Summary

The real NAS Mastodon/Rustodon interactions scenario reaches bidirectional
Like/Undo and delivery of a followers-only Rustodon Announce, then Rustodon's
unreblog endpoint returns HTTP 500. This is synthetic peer evidence, not a live
instance report or a result inferred from worker unit tests.

## Requirements

- Diagnose the existing failing peer case without weakening its assertions.
- Separate application SQL/queue behavior from fixture-role or harness defects.
- Add a focused red regression before any production correction; preserve exact
  Announce/Undo identity, privacy, recipient distribution and transactionality.
- No live replay, deployment, broader grant escalation or historical mutation.

## Acceptance Criteria

- Concrete cause identified with a bounded regression.
- Restricted-writer regression and real interactions scenario pass after a
  separately reviewed topical correction.

## Evidence

- `/srv/workspaces/rustodon-peer-tests/logs/interactions.log`:
  `/api/v1/statuses/<original-id>/unreblog` returned
  `500 {"error":"Internal Server Error"}` after successful private Announce.
- Run evidence:
  `source/target/peer-86911789209305125467036/` under the same workspace.
- Public, privacy and Note lifecycle peer scenarios passed independently.
- The Rustodon web log is empty; no SQLSTATE or precise cause is claimed yet.
- Follow-up to [NAS peer execution](adapt-and-run-peer-matrix-on-nas.md).

## Independent source diagnosis

- The peer uses the correct original-status ID. Using the wrapper ID would
  avoid removal rather than fix this failure; do not change the test that way.
- Checked-in writer grants cover the removal SQL, including durable-job DELETE.
  Successful Undo Like uses the same delivery-cancellation helper. No grant
  escalation is justified by current evidence.
- The HTTP mapper discards repository errors; a post-commit reload/serializer
  can also return 500. Existing cleanup did not retain the transaction-state
  discriminator or PostgreSQL error log, so the cause remains unconfirmed.
- Next bounded reproduction: one interactions run, retaining task-only PG errors,
  effective writer privileges, wrapper deletion/counters and correlated intent
  metadata before cleanup. Stop at first failure; no unnecessary body/token dumps.
