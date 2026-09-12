# Fix remote direct-message versus limited classification

## Summary

Remote Note classification never returns direct visibility 3; explicitly mentioned DMs become limited, serialize as private and miss direct-only conversation handling.

## Requirements

- Port pinned Mastodon explicit-mention versus silent-audience cases using real restricted-writer processing.
- Preserve access denial, provenance, followers/private/public semantics and silent-recipient behavior.
- Reuse existing peer privacy scenario; do not broadly rewrite stored historical rows.

## Acceptance Criteria

- Red regression proves explicitly mentioned direct messages currently classify incorrectly.
- Green checks cover direct versus silent-limited, REST visibility, conversation effects, unauthorized access and replay behavior.
- Existing peer direct scenario is executed later on combined source, or remains explicitly blocked.

## Notes

- Source: [test coverage audit](../test-coverage-audit.md).
- User authorized this implementation plan on 2026-09-12; no live deployment is implied.
