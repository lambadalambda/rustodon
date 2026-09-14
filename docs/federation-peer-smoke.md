# Isolated Mastodon peer smoke

The runner recognizes two exact host/physical-workspace profiles, but only the
NAS profile is currently authorized:

- NAS: `podman-worker`, `/srv/workspaces/rustodon-peer-tests/source`.
- Historical Secunda: `secunda`, `/home/lain/rustodon-parity/peer-tests`.

NAS uses the committed schema fixture and **all three cached pinned images**;
it neither fetches nor certifies a Mastodon source checkout. Secunda retains
its existing canonical source HEAD check. Dedicated source-contract and fixture
reproducibility gates remain separate.

For NAS, use the tooling container described in [the runbook](testing-on-nas.md)
with host networking, the rootful socket wrapper, and the workspace mounted at
the same absolute host path. `target/` and the run media directory must be
physical directories, not symlink aliases. Use a dedicated Cargo target per source root: shared compiled artifacts can
retain a different compile-time workspace marker and are correctly rejected. The runner verifies engine identity/rootful mode,
newly written host-visible files, network namespace, numeric run/database
markers, cached digests, and absence of its intended resources. A workspace
lock prevents concurrent peer runs. It refuses pulls/builds of container images;
provision missing exact pins separately through normal verified fixture tooling.

All five scenario commands below support both runner profiles. Use the NAS
profile unless the owner explicitly reauthorizes Secunda. Actual NAS results
live in [the owning issue](../meta/issues/adapt-and-run-peer-matrix-on-nas.md).
Pleroma is separate and remains unclaimed.

Historical Secunda invocation (do not run without explicit reauthorization):

```sh
ssh lain@secunda.local bash -s <<'REMOTE'
set -eu
cd /home/lain/rustodon-parity/peer-tests
CARGO_BUILD_JOBS=2 tools/federation-peer-smoke
REMOTE
```

## Scenario scope and retained evidence

This is bounded acceptance evidence, not the full interoperability matrix. All
five scenarios passed against the pinned Mastodon peer on the authorized NAS on
2026-09-13:

| Scenario | Result |
| --- | --- |
| `public` | Public discovery, follow, delivery, and signed Create passed. |
| `privacy` | Followers-only/direct delivery and outsider denial passed. |
| `notes` | Note create, update, delete, and visibility preservation passed. |
| `profile` | Profile text, flags, fields, and media URL updates passed. |
| `interactions` | Like/Undo and private Announce/Undo passed. |

The exact run IDs and logs are recorded in
[`adapt-and-run-peer-matrix-on-nas`](../meta/issues/adapt-and-run-peer-matrix-on-nas.md).
These runs predate the final browser API additions, do not include an explicit
reply scenario or simultaneous convergence stress, and do not establish Pleroma
or final-tree acceptance.

Run each scenario in a fresh invocation from the authorized NAS workspace:

```sh
CARGO_BUILD_JOBS=2 tools/federation-peer-smoke public
CARGO_BUILD_JOBS=2 tools/federation-peer-smoke privacy
CARGO_BUILD_JOBS=2 tools/federation-peer-smoke notes
CARGO_BUILD_JOBS=2 tools/federation-peer-smoke profile
CARGO_BUILD_JOBS=2 tools/federation-peer-smoke interactions
```

`privacy` creates followers-only and direct Notes in both directions. The normal
peer follows the author; a separate non-following recipient receives direct
Notes; a third account is an outsider. Assertions require received identity,
visibility, marker and signed inbox Create with the intended audience and no
Public address. Any observed Public-addressed Create attempt for that private
object is rejected, even if another private delivery succeeds. Receiving REST
checks require recipient 200, nonrecipient/outsider/anonymous 404; origin-side
outsider/anonymous access must also be denied. These REST probes do not fetch
the canonical ActivityPub status URL. No wire identifier is rewritten.

`notes` creates public and followers-only
Notes in each direction, waits for received state plus signed Create, edits
content and warning through the originating REST API, and requires Update to
change the same received row without changing URI or visibility. Private access
must remain denied after editing; Public-addressed Create/Update attempts are
rejected. Delete then requires both a signed Delete and retirement of that
previously observed row, followed by REST 404 for the former author/recipient.
The no-canonical-status-GET audit is checked throughout. Tombstone audience
privacy is not asserted. Ordinary `tag:` atomUri values are never altered by
the runner; parent fix `a3fe48a` is integrated.

After reciprocal follows, `profile` has each author PATCH text,
bot/locked/discoverable/indexable flags, a profile field, and both avatar/header
PNG uploads in one full API request. Request checks require both uploads and
descriptions; receiver checks bind the existing actor ID/URI, rendered note,
flags, field, and exact advertised media URLs to a signed inbox Update. Actor-URL
GETs after mutation are rejected through the end of the scenario, so refreshing
an actor cannot substitute for Update ingestion. URL convergence is not a claim
of successful remote image download.

`interactions` has each direction first receive a new public Note through push,
then Like and unlike it. The receiver must observe the correct favourite row
before its removal and a signed Undo whose object is the exact Like ID captured
from the wire—not a guessed ID scheme.
It then creates a followers-only **boost of that public Note**, requiring the
exact Announce ID, actor, target and followers audience in the same signed,
successful audit event; the original author
is also an established follower and must receive the private wrapper. Outsider
and anonymous REST access to the wrapper is denied on both sides. Undo must
retire that observed wrapper while leaving the public original active.

Announce envelope audiences are recorded separately from an embedded public
Note's audience, so embedding a public original does not falsely mark a private
boost public. A Public-addressed Announce attempt for this private boost fails
even if a private attempt succeeds. Fully read requests are audited before backend
forwarding with null status, then again with response status when available;
failed or in-flight forwarding cannot hide attempts, and only 2xx completion
events count as positive delivery evidence. Undo's outer audience is not constrained.
Status-GET and private-attempt audits are rescanned before each scenario ends.
The audit records IDs/audiences, never full private content or credentials;
`signed` records Signature-header presence, with received-state checks relying
on the application's real verification/worker path. Notification/counter parity,
boosts of other people's private Notes and concurrent interaction stress are
not claimed.

Each invocation verifies that its selected ignored test is listed before any
containers start; a renamed or missing case fails instead of silently passing an
empty selection. Future reruns must retain the task-owned cleanup checks and run
the shell/Python harness regressions in the same authorized environment. See
[Testing on NAS](testing-on-nas.md) for the current execution contract.

## What runs

- On NAS, cached pinned Mastodon 4.6.5, PostgreSQL and Redis images plus the
  committed schema fixture drive the database lifecycle. The runner neither
  pulls/fetches nor requires a Mastodon source checkout.
- The historical Secunda profile additionally requires its existing read-only
  `/home/lain/repos/rustodon/target/mastodon-v4.6.5` checkout to match
  `1440d55b139e39ec722c2a3db7f60b66cd889048` (canonical reference path:
  `/workspace/rustodon/target/mastodon-v4.6.5`).
- Two independently cloned, emptied schema databases. Rails creates three fresh
  functional local users (normal peer, recipient, outsider), OAuth tokens and
  signing keypairs on each side; no remote actors,
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

The public/privacy/profile scenarios have a three-minute overall deadline;
notes/interactions have six minutes. SQL statement and pool-acquisition limits
are two seconds; the
complete harness has a ten-minute deadline
with a final kill deadline after cleanup's grace period. This is sequential
peer convergence, not a simultaneous reciprocal-follow stress test. A trial
with overlapping follows hit a pinned Mastodon `account_stats` deadlock and
left pending requests after retry; that concurrency case is not claimed.
The initial peer-runner v2 search attempt returned no account results. Source
repair `0d513f5` now has passing named schema evidence in
`tools/mastodon-fixture schema-read-test v2_account_search`; the peer runner
retains v1 discovery, so these peer scenarios do not themselves prove v2 search
interoperability.

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
