# Address release-readiness review findings

## Summary

Resolve the correctness, security, and compatibility gaps found in the September 2026 codebase review before declaring v1 ready.

## Requirements

- Make synchronous local media creation and deletion recoverable across process or filesystem failure.
- Include the configured Paperclip root URL in ActivityPub actor media URLs.
- Fetch and process URI-only ActivityPub Create objects instead of acknowledging and dropping them.
- Bind browser CSRF protection to the authenticated session or otherwise prevent descendant-subdomain cookie injection.
- Add required first-party client compatibility probes, including announcements, to an independently derived route contract.
- Define and implement the intended custom-emoji federation behavior.
- Define the delivery semantics for mail jobs across SMTP acceptance and worker acknowledgement crashes.
- Tighten OAuth PKCE requirements for public clients.

## Acceptance Criteria

- Focused regression tests fail against the reviewed behavior and pass with each fix.
- Federation differential coverage compares actor media and exercises URI-only Create Note delivery.
- Media failure tests cover crashes or equivalent deterministic faults on both sides of database/filesystem publication and deletion.
- Security tests cover duplicate CSRF cookies from a descendant-domain injection scenario.
- The pinned frontend's ordinary startup requests have explicit supported or disabled contracts.
- Acceptance and release documentation accurately reflects the resulting evidence and remaining external proof gaps.

## Notes

- Live-peer convergence, mobile-client recording, production cutover/rollback, true disk exhaustion, sustained production load, and hard-power-loss evidence remain separate external acceptance work.

## Implementation

- Actor media roots, CSRF cookie injection, public-client PKCE, announcements,
  hashtag search, local media crash recovery, URI-only Create resolution, custom
  emoji federation, stable mail retry identity, and CI contract gaps are fixed.
- Focused unit, schema, worker, pinned-source, and Rails-versus-Rust differential
  coverage exercises each repaired boundary. Worker integration passes 49/49 and
  schema integration passes 37/37.
- The production Mastodon image does not include RSpec dependencies, so URI-only
  Create uses pinned Rails source/spec evidence plus executed Rust restored-fixture
  delivery coverage rather than a live Rails behavioral invocation.

## Remaining

- Keep this issue open until URI-only Create is exercised against a live pinned
  Rails runtime or the acceptance criterion is explicitly revised to accept the
  pinned-source contract plus restored-worker proof.

## Closed 2026-09-23

Closed by the user on 2026-09-23. The user accepts the pinned Rails source contract plus restored-worker delivery proof for URI-only Create, instead of a live Rails runtime run.
