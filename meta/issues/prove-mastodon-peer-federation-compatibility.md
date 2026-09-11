# Prove Mastodon peer federation compatibility

## Summary

Exercise the complete v1 federation profile against a pinned Mastodon 4.6.5 peer.

## Requirements

- Test discovery, follow, receive, reply, mention, like, boost, update, block,
  undo, and delete in both directions.
- Add malformed signature/digest, skew, size, redirect, SSRF, timeout,
  duplicate, and out-of-order cases.

## Acceptance Criteria

- Peer state converges without private/direct leakage or deleted-object resurrection.

## Progress

- Local federation behavior is covered by the 20-phase Rails-versus-Rust
  differential suite, signed transport and SSRF fixtures, durable worker
  integration (45/45), duplicate/out-of-order relationship tests, and cutover
  reopen verification against the pinned Mastodon 4.6.5 source.
- Live peer discovery, bidirectional state convergence, and peer-side
  idempotency remain unverified; this issue stays open until that external run
  is available.
- A 2026-09-11 live setup at `rustodon-lain.tunnel.eosrift.com` resolved and
  persisted `Gargron@mastodon.social`, proving Rustodon's outbound WebFinger and
  actor-fetch path. A temporary pinned Mastodon 4.6.5 peer then resolved the
  local WebFinger document, but its signed actor GET (key ID ending in
  `/actor#main-key`) received HTTP 503 from Rustodon during remote signer-key
  resolution. The peer therefore could not ingest the Rustodon account. No
  complete signature or secret was retained. This is the first live peer
  reproduction and the next investigation should isolate the signer-key
  resolution failure before attempting the bidirectional activity matrix.
- A separate live mention delivery to Pleroma 2.10.2 at `lain.com` isolated a
  concrete signer-key compatibility defect. Pleroma accepted the signed Create
  with HTTP 200 but its valid signed actor GET received HTTP 401 because its
  `internal.fetch` actor publishes a public-key PEM with an extra trailing LF;
  the RSA parser classified that otherwise valid key as corrupt. Rustodon now
  trims surrounding PEM whitespace at the centralized public-key parser, with
  regression coverage. An idempotent replay preserving the original activity
  ID then produced a Pleroma actor GET with HTTP 200, and `lain.com`'s public API
  exposed the imported status under the original Rustodon object URI with the
  intended local mention. No private key or complete signature was retained.
