# Accept ordinary ActivityPub tag atom identifiers

## Summary

The R10 receiver regression exposed another ordinary ingestion gap: locally serialized Notes can contain an opaque `tag:` atomUri, while the writer accepts only HTTP(S) aliases. Serialized Tombstones are also rejected by the inbox parser. Repair the optional legacy identifier handling without broadening ActivityPub actor/object authority.

## Requirements

- Reproduce serializer-produced tag atom identifiers through real Note ingestion and Tombstone deletion.
- Keep canonical ActivityPub object IDs HTTP(S), and prevent an optional alias from authorizing cross-account or cross-origin mutations.
- Use a media/metadata-specific helper rather than weakening every URI validator.
- Remove the R10 test's HTTP-URI isolation only once this distinct defect is proved red/green.

## Acceptance Criteria

- Ordinary serialized Create/Update/Delete with a tag atom identifier converge through the receiver worker.
- Malformed identifiers and cross-author/cross-origin alias attempts cannot change another actor's status.
- Targeted regressions and applicable aggregate gates pass on Secunda, with independent review.

## Notes

- Discovered while implementing R10. See `tests/workers/update_versions.rs` HTTP-URI isolation and `RemoteNoteData::parse`, `parse_delete`, and remote Delete alias handling.
- Existing R01–R18 review numbering is not silently rewritten; this is a separate follow-up found by executable integration.
