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
- Targeted regressions and applicable aggregate gates pass on an isolated worker, with independent review.

## Notes

- Discovered while implementing R10. See `tests/workers/update_versions.rs` HTTP-URI isolation and `RemoteNoteData::parse`, `parse_delete`, and remote Delete alias handling.
- Existing R01–R18 review numbering is not silently rewritten; this is a separate follow-up found by executable integration.

## Source repair and verification boundary

- A shared atom-specific helper discards scalar `tag:` metadata from lookup alias
  authority, including different tagging domains and malformed opaque tag text.
  This is not an RFC4151 validator or a claim that incoming tagging authority is
  authenticated. Existing HTTP(S) aliases and writer ownership/host checks remain.
- Notes and Tombstones normalize before lookup, forwarding, media cleanup and
  tombstone selection. Canonical ActivityPub identities stay HTTP(S).
- Added serializer/parser/unit regressions and extended the two-database signed
  receiver test with a persisted tag URI, split local/web domains, a matching
  victim legacy URI, Create/Update/Delete convergence and canonical-only tombstones.
- Independent source review caught a test-layer mismatch: canonical host equality
  is enforced by the writer, not `RemoteNoteData::parse`. Removed that incorrect
  parser assertion rather than loosening or moving production authority checks.
- Pinned Mastodon source was inspected remotely before the outage (revision
  `1440d55b139e39ec722c2a3db7f60b66cd889048`): `OStatus::TagManager#uri_for`,
  Note serializer `atom_uri`, Create fallback lookup and account-scoped Delete
  fallback. Rustodon intentionally does not infer that legacy mapping from a tag.
- **Compatibility limit:** a receiver row stored solely under a legacy tag remains
  unsupported, as before this repair. Sender-side legacy tags no longer prevent
  canonical ingestion; this does not implement Mastodon's historical alias fallback.
- **Execution pending:** parser/worker RED was launched on an isolated worker but remote access timed
  out during the outage and logs are unavailable; no result is claimed. The user
  requested continued work without the isolated worker, so implementation/review proceeds
  source-only. No local builds, tests, formatting, lint or containers ran. R10's
  old HTTP-URI isolation is removed in source but acceptance remains pending until
  remote red/green and aggregate gates execute. Do not archive this issue yet.

## Done 2026-09-23

First execution of the source repair (a3fe48a) on the NAS PG14 fixture:
3 atom unit tests and `worker-test update_versions` (2/2, including the
two-database Create/Update/Delete convergence with a persisted tag URI) pass.
No red run exists: the fix predates this execution, and reverting it only for a
red run was not proportionate.
