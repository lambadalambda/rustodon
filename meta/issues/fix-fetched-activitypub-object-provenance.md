# Fix fetched ActivityPub object provenance

## Summary

Prevent fetched Create and nested Announce wrappers from fabricating content attributed to another remote author.

## Requirements

- Bind embedded object authority to authenticated fetch provenance or independently dereference the canonical object.
- Preserve legitimate boosts whose announcer differs from the original author.

## Acceptance Criteria

- Adversarial cross-origin Create and nested Announce regression tests fail before the fix and pass afterward.
- No forged status, mention, notification, or boost is committed; legitimate remote boosts remain functional.

## Notes

- Findings R01 in [the essential feature parity review](../essential-feature-parity-review.md), baseline `1861d33`.
- Review-only discovery: no implementation fix is included in the review commit. End-to-end scenarios remain to be executed unless the report explicitly records a parser reproduction.
