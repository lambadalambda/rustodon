# Implement public federation discovery endpoints

## Summary

Expose the read-only discovery and ActivityPub representations that let remote
servers identify existing local accounts and retrieve their public content.

## Requirements

- Implement WebFinger for local accounts with canonical subject, aliases, and
  ActivityPub self link.
- Implement host-meta, NodeInfo discovery, and NodeInfo 2.0.
- Implement local actor JSON-LD with stable ID, type, username, profile URL,
  inbox, outbox, followers, following, shared inbox, and legacy RSA public key.
- Implement public local Note representation with attribution, addressing,
  replies, mentions, attachments, and canonical URLs.
- Implement basic paginated outbox, followers, and following
  `OrderedCollection` responses with privacy-aware contents.
- Negotiate ActivityPub content types correctly and keep ordinary HTML routing
  separable from JSON-LD responses.
- Do not implement inbox processing, remote fetching, or outbound delivery in
  this issue.

## Acceptance Criteria

- WebFinger round-trips every fixture local account to its exact actor ID.
- Actor and Note representations pass differential tests against Mastodon
  4.6.5 for the supported fields.
- JSON-LD responses include a valid ActivityStreams context and content type.
- Followers/following contents are hidden when the fixture account's policy
  requires it.
- Unknown, malformed, and unavailable WebFinger resources return compatible
  400, 404, and 410 outcomes.
- A locally started peer pinned to Mastodon 4.6.5 can resolve a fixture local
  account and fetch its actor and public status using a documented test case.

## Notes

- Depends on `implement-core-rest-serializers.md` and
  `implement-visibility-correct-account-status-reads.md`.
- HTTP signature verification, signed GET policy, inboxes, and delivery are
  separate federation-transport issues after this public discovery slice.
