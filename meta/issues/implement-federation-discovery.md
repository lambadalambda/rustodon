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
- Optional legacy signed-GET verification now protects existing actor reads;
  mandatory signed-GET policy, inboxes, remote-key fetching, and delivery remain
  separate federation-transport issues after this public discovery slice.

## Progress

- Implemented the supported discovery routes and serializers in
  `src/mastodon/activitypub.rs` and `src/web.rs`.
- Added the guarded `federation_discovery` differential case and focused helper
  tests; the differential case passes against the pinned fixture.
- Hardened reviewed boundaries for suspended actor serialization, explicit
  WebFinger authorities, numeric local quote URIs, suspended collection
  members, collection page presence, Accept quality values, and outbox query
  failures.
- Verified `mise run check`, targeted and full differential compatibility, and
  startup integration after the hardening pass. The guarded discovery case now
 covers WebFinger, host-meta, NodeInfo, signed and unsigned actor reads,
 public Note JSON-LD, paginated collections, privacy-aware members, and HTML
 negotiation against the pinned fixture.
- ActivityPub Note and local outbox pages now reuse the REST status authorization
  policy for optional OAuth or signed viewers. Followers can retrieve authorized
  private statuses, mentioned accounts can retrieve direct/limited statuses, and
  blocked viewers cannot receive public content; the guarded federation case
  covers username/numeric Notes and signed outbox pagination.
- Added the Rails status collection surface for username and numeric ActivityPub
  routes: standalone `replies` pages serialize local Notes and remote URI items,
  while `likes` and `shares` expose status-stat totals. Parent status policy is
  applied before every collection response, and the guarded case covers
  pagination, nullable local reply URIs, private/direct access, and blocked
  viewers.
- Boost object GETs now redirect to the original status, while boost collection
  URLs remain on the status route without the `/activity` suffix. Status object
  and activity responses emit Mastodon-compatible alternate `Link` headers.
- Status object responses now match Rails cache policy: distributable public
  statuses use a three-minute public cache in normal fetch mode, while private
  status objects and authorized-fetch responses remain non-shareable. Activity
  responses preserve Rails' three-minute private cache behavior for
  non-distributable statuses.
- Status errors and boost redirects now receive the Rails private cache default
  and the corresponding public or authorized-fetch `Vary` header before they
  leave the status route.
- Browser-cookie sessions now authorize ActivityPub status and collection reads
  with the same precedence as Rails' web session, while limited federation still
  requires a valid request signature.
- Public distributable status Notes with pending quotes now use Rails'
  five-second response cache window; Activity objects retain the three-minute
  cache window.
- Public-fetch status reads now treat failed optional signatures as anonymous,
  matching Rails while preserving errors in limited/authorized-fetch mode.
- WebFinger URL resources now recognize mixed-case HTTP(S) schemes, matching
  Rails' resource parser; the guarded differential case covers the behavior.
- Note serialization now includes reply Atom URIs and conversation/context
  metadata, including the local OStatus fallback for reply parents; the same
  metadata is carried by outbound worker Notes.
