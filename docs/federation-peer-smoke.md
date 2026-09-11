# Isolated Mastodon peer smoke

Run **only on Secunda**, from the task workspace:

```sh
ssh lain@secunda.local bash -s <<'REMOTE'
set -eu
cd /home/lain/rustodon-parity/peer-tests
CARGO_BUILD_JOBS=2 tools/federation-peer-smoke
REMOTE
```

This is a bounded first smoke, not the full interoperability matrix. Pleroma,
private audiences, edits/deletes, media and lifecycle convergence remain future
work. See [the open issue](../meta/issues/add-isolated-federation-peer-tests.md)
for actual passing/blocked evidence.

## What runs

- Cached pinned Mastodon 4.6.5, PostgreSQL and Redis images and the database
  lifecycle from `tools/mastodon-fixture`. No image pull or source fetch. The
  existing read-only `/home/lain/repos/rustodon/target/mastodon-v4.6.5` checkout
  must match `1440d55b139e39ec722c2a3db7f60b66cd889048` (canonical reference path:
  `/workspace/rustodon/target/mastodon-v4.6.5`).
- Two independently cloned, emptied schema databases. Rails creates a fresh
  local user, OAuth token and signing keypair on each side; no remote actors,
  follows, statuses or signing keys are copied between peers. Separate media
  roots and origins: `mastodon.peer.invalid` and `rustodon.peer.invalid`.
- Mastodon Puma **and Sidekiq**, Rustodon web **and durable workers**. Rustodon
  uses the documented separate least-privilege runtime/writer roles and explicit
  operational migration. Mastodon runtime uses a separate non-superuser with
  privileges only in its own database (including TEMP for materialized-view
  refresh). Setup/database assertions use the disposable owner. The two Rails
  bootstrap processes use separate Redis database numbers.
- Two loopback TLS forwarders with a fresh one-day CA. Original URLs, Host,
  signature material and TLS server-name verification remain unchanged. They
  forward only to their fixed local web backend, reject other Host values and
  transfer-encoded requests, and bound bodies/timeouts.

## Test-only routing

Rustodon's explicit origin map and CA are enabled together through
`RUSTODON_TEST_PEER_ORIGINS` and `RUSTODON_TEST_PEER_CA`, only in debug builds
with `test-support`. Every unmapped destination fails closed. The harness uses
only loopback high-port endpoints for two `.invalid` HTTPS origins. This is not
an ordinary release SSRF exemption, DNS/public-IP spoof, proxy tunnel, or TLS
verification bypass.

A small initializer is mounted **only in the disposable Mastodon image**. It
maps the same exact two HTTPS origins to loopback sockets while retaining
HTTP.rb TLS/signing/response handling. `SSL_CERT_FILE` explicitly selects the
fresh CA. The initializer never changes the pinned source checkout. No private
address exception is installed in either application's production configuration.

## Assertions and bounds

`tests/federation_peers.rs` is ignored by default. It validates task root and
both database comments before mutation, then:

1. Confirms no cached remote accounts; searches each fresh cross-peer account
   through `/api/v1/accounts/search?resolve=true` and checks the received actor
   URI and signing key (including Mastodon's current `keypairs` storage).
2. Follows each direction in sequence through the APIs. Polls both databases
   for that direction's sender/receiver rows and absence of pending requests
   before starting the reverse direction (60 attempts per direction).
3. Creates one public status per peer through its API, then polls only the
   receiving database for the exact object URI, actor URI, public visibility,
   remote flag and content marker (60 attempts). **No status URL is fetched or
   resolved by the test**. Both direction results are printed.
4. Requires a successful **signed inbox Create** carrying the matching actor,
   object and public audience in each TLS forwarder's identity-only audit, and
   rejects any recorded GET of either new status URL.

The scenario has a three-minute overall deadline and two-second SQL statement
and pool-acquisition limits; the complete harness has a ten-minute deadline
with a final kill deadline after cleanup's grace period. This is sequential
peer convergence, not a simultaneous reciprocal-follow stress test. A trial
with overlapping follows hit a pinned Mastodon `account_stats` deadlock and
left pending requests after retry; that concurrency case is not claimed.
Rustodon's v2 search currently returns no account results, so this smoke does
not claim v2 account-search parity either.

HTTP 2xx is never reported as successful convergence. Workers must be ready
before scenarios start. Unsupported ingestion fails the test; do not preseed
remote rows, call a resolver for the new status, or inject activities to make it
pass. Coordinate application fixes with their owning parity issues.

## Resources and evidence

One task-owned PostgreSQL volume/network, Redis, sequential seed containers,
Puma/Sidekiq containers, and four host processes. Rails containers are capped at
1 CPU, 1200 MiB and 256 PIDs; Ruby/Rust worker concurrency is two and Cargo uses
two jobs. All publication is loopback; Mastodon containers use the host network
only to reach the loopback test endpoints, not any instance environment.

EXIT/signal cleanup removes only recorded containers, PostgreSQL volume/network
and host PIDs. Logs remain under `target/peer-<pid>/`; generated CA/server private
keys and certificates are removed. The harness never prunes images/volumes or
uses `rsync --delete`. A SIGKILL cannot execute cleanup: in that case, remove
only the exact `peer-<pid>` and `rustodon-fixture-...-<pid>` resources recorded by
the run, never a broad prune. Do not synchronize ignored `target/`, `.git`,
`.local-instance*` or `.env*` content.
