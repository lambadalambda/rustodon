# Prove concurrent relationship writes

## Summary

Prove that concurrent local relationship requests converge on Mastodon's single
relationship, counters, notifications, and response shape.

## Requirements

- Exercise concurrent follow, block, and mute requests against the same account
  pair.
- Preserve one relationship row, stable counters, and retry-safe teardown.
- Compare Rustodon and Mastodon responses and persisted state, including
  synchronous notification cleanup, then restore both fixture databases.

## Acceptance Criteria

- Guarded differential coverage runs the concurrent cases on both HTTP targets,
  accepts Mastodon's unique-validation loser response where applicable, compares
  compatible final state, and restores every touched relationship, notification,
  request, and counter row.

## Progress

- Added concurrent follow, block, and mute HTTP operations to the relationship
  differential case. Stable relationship options, counters, block/mute
  notification state, and full fixture restoration are compared; asynchronous
  follow notification delivery remains outside this proof.
- The focused and full 10-case differential suites pass. Remote federation
  transitions and durable outbound activity delivery remain covered by the
  parent relationship and federation issues.
