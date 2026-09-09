# Implement safe remote fetching

## Summary

Resolve remote WebFinger, actors, keys, statuses, and bounded media safely.

## Requirements

- Enforce canonical IDs/origins, content types, redirects, DNS/private-address
  SSRF protection, timeouts, response/decompression bounds, and signed GET.
- Cache bounded remote images compatibly while retaining text on media failure.

## Acceptance Criteria

- Adversarial redirect, rebinding, oversized, timeout, and identity fixtures
  fail closed without internal network access.

## Progress

- Added a bounded `RemoteFetcher` with strict HTTP(S) URL validation, disabled
  proxy use and automatic redirects, fixed DNS address pinning, redirect
  revalidation, connect/request timeouts, explicit content-type checks, and
  streaming response-size bounds.
- Added Mastodon-compatible private/documentation/reserved address rejection,
  including mixed DNS answers, IPv4-mapped IPv6 addresses, and IPv4-compatible
  IPv6 addresses. Pure policy tests cover the SSRF and content-type boundaries.
  ActivityPub profile parameters,
  exact `200 OK` status, canonical JSON `id` matching, compressed-response
  rejection, and hard configuration ceilings are explicit.
- The fetcher is now wired into exact WebFinger/account resolution, but remains
  separate from media retrieval; deterministic adversarial GET and identity
  fixtures cover the bounded transport path without opening internal network
  connections.
- Added a typed WebFinger/ActivityPub actor resolver layer that requires a
  matching subject, ActivityStreams context, canonical actor origin and ID,
  supported actor type, safe endpoint shapes, profile-host binding, bounded
  actor fields, and actor-owned public keys. Exact remote account
  search and lookup can now enforce normalized domain allow/block policy, reuse
  fresh stored accounts, refresh stale WebFinger data, apply process-local
  IP/handle throttles, and transactionally upsert the minimal account identity
  by canonical actor URI. Same-origin redirects are required before following
-  them. Host-meta fallback is origin-bound, signed actor/key GETs include
  Mastodon's `Host`, `Date`, and `(request-target)` coverage and are re-signed
  for each validated redirect, and actor key entries may be embedded or
  bounded references. Referenced key documents are fetched without fragments,
  checked against the requested key ID and actor owner, and persisted with
  transactionally reconciled keypair rows. A bounded aggregate resolution
  deadline prevents multiple key references from multiplying request latency;
  unavailable individual references are skipped. Inbound signature verification
    now performs bounded on-demand remote-key refresh with owner/WebFinger
    confirmation, limited-federation/domain policy checks, stale-key retry, and
    verify-before-persist behavior. A per-canonical-host in-flight budget now
    covers resolver, key, media, and delivery fetches; durable operational leases
    coordinate independent web and worker pools while the process-local fallback
    remains available when no operational pool is configured.
- Signed ActivityPub POST transport now shares the bounded DNS, redirect,
   timeout, encoding, and response-size protections. Request bodies are bounded
   before signing or network access. Deterministic POST fixtures cover mixed DNS
   answers, redirect-hop rebinding, same-origin 307/308 replay, permanent
   redirect rejection, timeout, and oversized responses.
- Added bounded remote media job processing and proxy retrieval; follow-up
  authorization and account-domain policy hardening is recorded below.
  - Media jobs now use the remote account domain for policy decisions, and the
  proxy reuses REST status authorization for public, private, direct, and
  limited statuses. Bounded image processing, Paperclip recovery, retry
    classification, and failed-media text retention are covered; the restored
    fixture now exercises a successful HTTP media response and cached Paperclip
    writes.
  - The media proxy now serves verified cached remote Paperclip original and small
     files before refetching, including the PNG content type for GIF small styles.
     A deterministic feature-gated HTTP fixture now exercises the real bounded
     worker fetch path, including cached GIF original and PNG small files.
     The fixture endpoint is refused in release builds so production SSRF
     protections remain the only available network path.
  - Added deterministic local HTTP transport fixtures for the real fetch path:
     oversized bodies are rejected before persistence, cross-origin redirects fail
     with an origin error, and stalled responses hit the bounded request timeout.
     Existing address-policy and ActivityPub identity fixtures continue to prove
     rebinding and canonical-ID rejection without opening internal connections.
   - The same debug-only endpoint mechanism now exercises signed ActivityPub POST
      transport through the durable worker without weakening production DNS or
      private-address validation.
   - Signature-refresh cooldowns now coordinate through the shared PostgreSQL
      rate-limit table, while retaining the process-local fallback when no
      shared pool is configured. Independent pools, client isolation, and
      expiry cleanup are covered by the operational fixture.
    - Expired remote-fetch leases are reclaimed during acquisition and routine
       maintenance. Explicit release, cancellation cleanup, independent-pool
       coordination, and lease expiry are covered by the operational fixture.
    - Test-only endpoint routing now applies the production resolved-address policy
      instead of bypassing it. Mixed DNS answers are rejected before connection,
      and a redirect fixture rechecks the full answer set on the next hop, proving
      rebinding protection without opening a loopback connection. The local
      checks, worker/schema integrations, startup integration, and guarded
      differential suite pass.
