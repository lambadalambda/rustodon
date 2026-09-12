# Suppress semantically unchanged inbound edit effects

## Summary

A genuinely newer inbound Update that sanitizes to unchanged content still changes edited_at and emits edit effects, unlike pinned Mastodon.

## Requirements

- Port pinned sanitized-equivalent HTML Update regression with a genuinely newer timestamp.
- Compare the meaningful fields relevant to current supported behavior; preserve real content/media/poll changes and metadata reconciliation.
- Avoid spurious edit notifications/streams while retaining timestamp/idempotency/privacy fences.

## Acceptance Criteria

- Red proves unchanged rendered content creates an edit effect.
- Green proves equivalent updates do not mark edited or emit edit effects; meaningful updates still do, and old/duplicate/implicit update behavior remains covered.

## Notes

- Source: [test coverage audit](../test-coverage-audit.md).
- User authorized this implementation plan on 2026-09-12; no live deployment is implied.
