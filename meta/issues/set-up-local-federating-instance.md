# Set up a local federating Rustodon instance

## Summary

Provision a persistent local Rustodon instance and expose it through a stable
HTTPS origin so it can participate in ActivityPub federation.

## Requirements

- Use the pinned Mastodon 4.6.5-compatible PostgreSQL schema and local Paperclip media.
- Keep PostgreSQL and Rustodon data local to this system.
- Run the Rustodon web and worker processes with least-privilege database roles.
- Expose only the Rustodon HTTP service through a stable HTTPS hostname.
- Create a local user with non-fixture credentials.
- Document the operational lifecycle without publishing credentials or private locators.

## Acceptance Criteria

- Rustodon preflight succeeds with the runtime and writer configuration.
- The worker reports ready and the local `/health` and `/ready` endpoints succeed.
- The public HTTPS origin serves healthy WebFinger, NodeInfo, and ActivityPub actor endpoints.
- A remote fediverse server can resolve the local account, or any external federation blocker is recorded with evidence.
- Persistent data survives a controlled service restart.

## Notes

- Rustodon currently supports cutover from Mastodon 4.6.5 rather than greenfield schema creation, so bootstrap may require the pinned Mastodon image.
- The public hostname becomes part of permanent ActivityPub identities and must remain stable.
- The service is experimental and public exposure must not include PostgreSQL or administrative infrastructure.

## Setup Evidence (2026-09-11)

- The stable public origin exposed only the web process; PostgreSQL and Redis
  remained internal to the Podman network.
- `rustodon preflight` passed with only the expected SMTP-disabled and no-persisted-tag-URI warnings.
- Worker readiness reported `ready=true`, scheduler alive, no missing lanes, no queued jobs, and no dead letters.
- Local and public `/health` and `/ready`, WebFinger, NodeInfo, and the ActivityPub actor document passed. Browser password login reached the authenticated Mastodon shell.
- Rustodon successfully resolved and persisted `Gargron@mastodon.social`, proving outbound WebFinger and actor fetching.
- A controlled stop/start retained the same local account and actor identity and
  returned to worker, local, and public readiness.
- A mode-restricted PostgreSQL/role/media/credential backup was created;
  restoration has not yet been tested.
- A temporary pinned Mastodon 4.6.5 peer attempted signed resolution of the local
  account. WebFinger succeeded, but Rustodon returned HTTP 503 for the peer's
  signed actor GET while resolving the peer instance-actor key, so Mastodon could
  not ingest the account. This is the external federation blocker; keep this
  setup issue open alongside [Prove Mastodon peer federation compatibility](prove-mastodon-peer-federation-compatibility.md).
- Pleroma 2.10.2 at `lain.com` accepted an outbound mention Create and fetched the local actor after Rustodon was fixed to accept its trailing-whitespace public-key PEM. The peer's signed actor GET returned HTTP 200, and its public API exposed the imported status under the original Rustodon object URI. The pinned Mastodon 4.6.5 HTTP 503 remains a separate blocker.

## Closed 2026-09-23

Closed 2026-09-23. All acceptance criteria are met: Pleroma resolved the account, and the Mastodon HTTP 503 blocker was fixed before the 2026-09-13 peer `public` scenario passed (discovery and follow in both directions). rustodon.social is the live instance.
