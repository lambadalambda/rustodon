# Implement HTTP signatures and digests

## Summary

Implement Mastodon's draft Cavage RSA-SHA256 interoperability profile.

## Requirements

- Sign and verify GET/POST requests using compatible account/keypair keys.
- Verify exact body SHA-256 digests, required signed headers, key ownership,
  and clock skew.

## Acceptance Criteria

- Mastodon-derived vectors and malformed signature/digest cases pass; newer
  signature systems remain explicitly unsupported.

## Progress

- Added a reusable legacy Cavage `rsa-sha256` signer/verifier in
  `src/mastodon/signatures.rs` with exact `(request-target)` construction,
  queryless-signature compatibility fallback, `Date`/`expires` clock windows,
  SHA-256 body digests, required-header checks, explicit key-ID binding, and
  deliberate RFC 9421/`hs2019` rejection.
- Added fixed Mastodon GET and POST vectors plus malformed, tampered-body,
  duplicate-parameter, spacing, stale-date, key-binding, and redaction tests.
- The pure primitive is covered by the full local check and guarded Rails
  differential suite. The issue remains open until transport integration,
  full ownership/revocation/domain policy, remote-key fetching, and outbound
  request signing are implemented.
- Added persisted public-key resolution for canonical actor key IDs and legacy
  `acct:` aliases, preserving account ownership plus revoked/expiry metadata.
  Existing actor GET endpoints now optionally verify legacy signatures before
  serialization; the signed actor request passes the pinned Rails differential
  case. Remote actor resolution now fetches bounded embedded or referenced key
  documents with owner/ID checks. Inbound actor signature verification now
  refreshes stale or missing remote keys once, verifies fresh material before
  persistence, preserves key revocation/expiry state, and requires signatures
   in limited-federation mode. Inbox enforcement, outbound signing, and broader
   signed-GET policy remain separate transport work.
- Added bounded signed ActivityPub JSON POST transport to `RemoteFetcher`.
  Host, Date, Digest, and `(request-target)` are signed for each request;
  request bodies are bounded, only same-origin 307/308 redirects are followed,
  redirects are re-signed, and successful 2xx delivery responses remain
  bounded and identity-encoded. Outbound activity fan-out and delivery retry
  classification remain with the outbound-delivery issue.
- The issue acceptance criteria are satisfied: Mastodon-derived GET/POST vectors,
  malformed signature/digest cases, ownership/revocation checks, clock windows,
  inbound enforcement, and bounded signed transport all pass the full local
  gate. Broader delivery policy remains scoped to its owning issue.
