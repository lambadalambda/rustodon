# Adapt and run the existing peer matrix on NAS

## Summary

Existing peer scenarios are locked to a Secunda host/path and expanded privacy/lifecycle scenarios lack execution evidence.

## Requirements

- Narrowly adapt the runner for explicit authorized NAS workspaces and verified existing pinned source/image prerequisites; keep isolation/TLS/audience audit guards.
- Run existing public, privacy, notes, profile and interactions scenarios, preserving received-state and no-fetch/audience evidence.
- Keep Pleroma build/peer evidence separate; no floating pins, credentials workaround, live peers or broad cleanup.

## Acceptance Criteria

- Runner guard/source validation tests pass before any actual scenario execution.
- Each Mastodon scenario has a clear pass/fail/blocked result on combined source; failures are diagnosed and tracked without weakening assertions.
- Pleroma prerequisite and scenario status is recorded separately, not claimed from Mastodon-only success.

## Notes

- Source: [test coverage audit](../test-coverage-audit.md).
- User authorized this implementation plan on 2026-09-12; no live deployment is implied.
