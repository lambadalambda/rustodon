# Audit everyday Mastodon frontend parity

## Summary

Sweep Rustodon for user-visible differences from Mastodon that appear during ordinary use of the bundled web frontend, prioritizing common workflows over obscure API compatibility edge cases.

## Requirements

- Exercise the basic integrated frontend journeys: sign-in/startup, home and public timelines, composing posts, media, replies, boosts, favourites, bookmarks, notifications, profiles, search, conversations, and common preferences.
- Compare frontend requests and behavior with the Mastodon 4.6.5 contracts already pinned in the repository.
- Prioritize missing, broken, stale, or materially different behavior that ordinary interactive users are likely to encounter.
- Exclude speculative edge cases and low-usage standalone API differences unless they directly break the bundled frontend.
- Record each actionable finding as a focused repository issue with reproduction evidence and expected behavior.

## Acceptance Criteria

- Representative everyday workflows are covered by browser checks, existing parity matrices, or focused source/contract inspection.
- Findings are ranked by user impact and confidence, with duplicates linked to existing open issues rather than recreated.
- Any new high-confidence gaps have focused issue files and open-index entries.
- The sweep records what was tested, what could not be exercised, and where coverage remains uncertain.

## Notes

- This is a discovery and triage sweep, not a mandate to fix every finding in one change set.
- Prefer a short list of reproducible everyday problems over a large list of theoretical differences.
