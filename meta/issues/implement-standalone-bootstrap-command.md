# Implement the standalone instance bootstrap command

## Summary

Add a fail-closed Rustodon installer command that turns a fresh PostgreSQL 14
database plus empty local media directory into a usable standalone instance.

## Requirements

- Install the committed compatible public schema with a fresh `timestamp_id`
  salt and exact migration ledger.
- Seed only production baseline data: standard roles, reserved username policy,
  fresh instance actor/signing key, settings, and a confirmed first Owner.
- Install Rustodon operational state, refresh functions, and exact runtime and
  writer grants without requiring Mastodon credentials or processes.
- Read the first-admin password without exposing it in process arguments by
  default; never print generated keys or secrets.
- Reject non-empty, partial, drifted, unsupported, or conflicting databases and
  non-empty/unsafe media roots.
- Preserve the existing cutover and ordinary `create-user` behavior.

## Acceptance Criteria

- The command is safe on a fresh target, fails closed on conflicting state, and
  does not rotate identity or credentials on an exact verification rerun.
- Runtime and writer connections pass startup/preflight after installer
  credentials are removed.
- The first administrator can authenticate and has the Owner role.
