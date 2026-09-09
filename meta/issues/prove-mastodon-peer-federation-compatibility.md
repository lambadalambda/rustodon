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
  integration (43/43), duplicate/out-of-order relationship tests, and cutover
  reopen verification against the pinned Mastodon 4.6.5 source.
- Live peer discovery, bidirectional state convergence, and peer-side
  idempotency remain unverified; this issue stays open until that external run
  is available.
