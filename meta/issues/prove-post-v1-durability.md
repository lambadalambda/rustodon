# Prove post-v1 durability under disk-full, power loss and load

## Summary

Moved out of [harden-v1-release](harden-v1-release.md) on 2026-09-23. For a
single-instance prototype these proofs are not proportional to the risk
(AGENTS.md, "Prototype operating mode"). Backups, rollback images and the
existing crash-recovery and resource-limit tests cover v1.

## Requirements

- True disk-full / quota exhaustion for database and media writes.
- Hard power loss during worker and media transactions.
- End-to-end sustained load for 1-20 users.

## Acceptance Criteria

- Each case has a repeatable harness and a recorded result, or is explicitly
  declined.
