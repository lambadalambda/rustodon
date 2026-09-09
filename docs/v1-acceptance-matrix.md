# Rustodon v1 Acceptance Matrix

This document is the traceability index for the authoritative requirements in
[`v1-scope.md`](v1-scope.md). It distinguishes local automated evidence from
acceptance work that still needs a browser, mobile client, live Mastodon peer,
or a cutover rehearsal. An open row must not be described as complete.

## Status

- `A`: automated evidence exists in the repository and is part of a named gate.
- `M`: implementation exists, but the acceptance case still requires an
  explicit operator, browser, or client run.
- `O`: implementation or required evidence remains open.
- `D`: deliberately deferred by the v1 scope.

## Executable Gates

| Surface | Contract and proof | Status |
| --- | --- | --- |
| REST inventory | `API_ROUTE_INVENTORY` contains every advertised method/path, rejects duplicate method/path pairs, and `V1_REQUIRED_API_ROUTES` checks every required v1 route. Tests: `web::tests::api_route_inventory_is_unique_and_declares_protocol_contracts`, `web::tests::v1_required_api_routes_are_inventoried_with_explicit_support`. | A |
| CLI inventory | `tests/cli_help.rs` checks the process modes and every administrative command exposed by `rustodon --help` and `rustodon admin --help`. | A |
| Differential compatibility | `mise run differential` runs 18 general Rails-versus-Rust cases plus isolated notification-write and status-authorization phases against independent Mastodon and Rustodon database/media clones and compares responses and state. The complete suite currently passes; `mise run differential -- <case>` runs one case. | A locally; peer convergence remains open |
| Authorized-fetch reads | `mise run differential -- authorized_fetch_read_routes_require_signatures` starts Rustodon with limited federation enabled and checks protected actor, Note, activity, outbox, followers, and following routes reject unsigned reads while `/actor` remains public. | A locally |
| Fixture and schema | `mise run fixture-verify`, `mise run mastodon-schema-integration`, and `mise run operational-schema-integration`. | A |
| Fixture cutover and rollback | `mise run cutover-integration` migrates the isolated operational schema, starts Rustodon, runs the operator smoke, reopens the pinned Mastodon web process, and compares stable public state and media. | A locally; live production rehearsal remains open |
| Workers | `mise run worker-integration` covers enqueue, leases, cancellation, retries, dead letters, resource limits, readiness, shutdown, and crash recovery. | A locally; peer convergence remains open |
| Startup and preflight | `mise run startup-integration` and `mise run preflight-integration`. | A |
| Operator smoke | `tools/rustodon-smoke` checks health, readiness, authentication, reads, a no-op write, media, WebFinger, and actor discovery against a running instance. | A locally; live operator run remains open |

## Supported Commands

The web process, worker, and preflight modes are covered by the root CLI help
test. The following administrative commands are the complete current command
surface; their help entries are checked by
`tests/cli_help.rs::admin_help_exposes_operational_schema_migration`.

| Command | Purpose | Status |
| --- | --- | --- |
| `rustodon web` | Serve REST, web, federation, media, and streaming requests. | A |
| `rustodon worker` | Process durable background work. | A |
| `rustodon preflight` | Validate a Mastodon cutover without mutation. | A |
| `rustodon admin migrate-operational-schema` | Create or upgrade Rustodon-owned operational tables. | A |
| `rustodon admin worker-readiness` | Inspect lane, scheduler, queue, and dead-letter readiness. | A |
| `rustodon admin dead-jobs --limit N` | List bounded dead-letter metadata. | A |
| `rustodon admin reset-password` | Replace a user's password and revoke sessions/tokens. | A |
| `rustodon admin create-user` | Create a confirmed local user or queue confirmation mail. | A |
| `rustodon admin resolve-report` | Resolve or reopen a report. | A |
| `rustodon admin delete-status` | Delete a local status with moderation authorization. | A |
| `rustodon admin reconcile-account-stats` | Repair denormalized account counters. | A |
| `rustodon admin suspend-account` | Suspend a local or remote account. | A |
| `rustodon admin unsuspend-account` | Remove a moderation suspension. | A |
| `rustodon admin block-domain` | Create or update a global domain policy. | A |
| `rustodon admin unblock-domain` | Remove a global domain policy. | A |
| `tools/rustodon-smoke` | Run non-destructive operator smoke checks against a live origin. | M |

## Supported API Routes

`src/web.rs::API_ROUTE_INVENTORY` is the complete advertised API surface: 104
canonical method/path contracts, with trailing-slash aliases registered by the
router but not duplicated in the inventory. The inventory and route-contract
tests check unique method/path pairs, representative authentication and
pagination declarations, while response-finalization tests check cache
behavior and the explicitly disabled translation response. The
`V1_REQUIRED_API_ROUTES` test checks the required v1 subset against that same
inventory, so adding a required route without a support declaration fails the
test suite. The inventory also contains compatibility reads, frontend probes,
and some optional endpoints implemented ahead of the v1 boundary; those routes
are not v1 acceptance evidence and must not be treated as required features.

## Scope Traceability

The identifiers below are deliberately stable labels for the bullets in
[`v1-scope.md`](v1-scope.md). The implementation and proof column names the
owning issue, code surface, or acceptance command.

### Startup, Safety, and Cutover

| ID | Requirement | Implementation and proof | Status |
| --- | --- | --- | --- |
| START-01 | Parse Mastodon domains, PostgreSQL, local media, SMTP, proxy, and encryption settings. | `src/config.rs`; `tests/config.rs`; `mise run preflight-integration`. | A |
| START-02 | Verify schema, functions, sequences, keys, media root, and domains. | `src/startup.rs`, `src/preflight.rs`; startup and preflight integration. | A |
| START-03 | Provide `rustodon preflight`. | `src/main.rs`; `tests/cli_help.rs`; preflight integration. | A |
| START-04 | Refuse object storage, unknown schema, unreadable keys, unsupported jobs, and unsafe media layouts. | `src/preflight.rs`; rejection matrix in `tests/preflight.rs`. | A |
| START-05 | Provide health and worker/readiness checks. | `/health`, `/ready`, `AdminCommand::WorkerReadiness`; startup and worker integration. | A |
| START-06 | Preserve proxy-facing paths and trust forwarded headers only from configured proxies. | `src/web.rs`; request metadata tests. | A |
| START-07 | Document snapshot, drain, cutover, smoke, and rollback. | [`cutover.md`](cutover.md); `mise run cutover-integration` rehearses Rustodon startup, shutdown, Mastodon web reopen, Rails verification, and preservation checks. | A locally; live production rehearsal remains open |
| START-08 | Detect pending scheduled statuses, active polls, pending deletions, WebAuthn-only users, Sidekiq work, relays, object storage, and SSO. | `src/preflight.rs`; preflight diagnostic tests and integration cases. | A |

### Authentication and OAuth

| ID | Requirement | Implementation and proof | Status |
| --- | --- | --- | --- |
| AUTH-01 | Accept existing bearer tokens with revocation, expiry, owner-state, and scope checks. | `src/mastodon/auth.rs`; `tests/oauth.rs`; differential `oauth_bearer_authentication`. | A |
| AUTH-02 | Password login and logout for existing users. | Rust-owned browser auth routes in `src/web.rs`; lifecycle parity is covered by differential `browser_authentication`, while live browser acceptance remains required. | M |
| AUTH-03 | Existing TOTP and backup-code verification; WebAuthn is not required. | `src/crypto.rs`, `src/mastodon/auth.rs`, browser authentication/settings handlers, auth tests, and guarded `browser_authentication`/`browser_two_factor_management` cases. | M; local fixture proof complete, live browser evidence remains open |
| AUTH-04 | OAuth authorization code, PKCE, and revocation. | `src/mastodon/oauth.rs`, `src/web.rs`; differential `oauth_authorization_code` and browser lifecycle guard coverage; OAuth tests. | A |
| AUTH-05 | Dynamic app registration and application credential verification. | `POST /api/v1/apps`, `/api/v1/apps/verify_credentials`; differential OAuth coverage. | A |
| AUTH-06 | Password reset, confirmation email when SMTP is configured, and administrator reset. | `src/mail.rs`, browser reset routes, `admin reset-password`; `tests/mail.rs` and CLI tests. | M |
| AUTH-07 | Rust-owned browser session format; Rails cookie compatibility is not required. | Browser session handlers and lifecycle session tests in `src/mastodon/repository.rs` and `tests/differential/writes.rs`; browser run remains open. | M |

### Web Client

| ID | Requirement | Implementation and proof | Status |
| --- | --- | --- | --- |
| WEB-01 | Reuse exact pinned Mastodon frontend assets. | `public/packs/BUILD.md`; fixture provenance and checksum verification. | A |
| WEB-02 | Render the HTML shell, initial state, CSRF data, VAPID metadata, and mount point. | `src/web.rs`; differential `local_web_client_shell`. | A |
| WEB-03 | Serve packs, themes, locales, service worker, and uploaded files. | Frontend asset routes and Paperclip routes; shell/media tests. | A |
| WEB-04 | Provide login, reset, and essential account settings UI. | Rust-owned `/auth/*` and `/settings/*` routes, including encrypted TOTP/recovery-code management; browser flow recording remains open. | M |
| WEB-05 | Advertise unsupported optional features as disabled and return stable empty probes. | Disabled route contracts, including translation languages; route inventory test. | A |

### Core REST API

| ID | Requirement | Implementation and proof | Status |
| --- | --- | --- | --- |
| REST-01 | Decimal IDs, timestamps, HTML, nullable keys, pagination, CORS, scopes, and error conventions. | `src/mastodon/rest`, `src/web.rs`; differential `rest_protocol_contracts` and serializer tests. | A |
| REST-02 | Instance and identity endpoints. | Required-route contract plus account/instance serializers and differential cases. | A |
| REST-03 | Status lifecycle, visibility, replies, mentions, media, interactions, history, context, and idempotent creation. | `WriteRepository`, REST handlers, differential write cases, schema and worker integration. | A locally; complete client/peer proof remains open |
| REST-04 | Timelines, collections, filters, markers, conversations, and relationship lists. | Repository projections, route inventory, schema/differential tests. | A locally; complete client proof remains open |
| REST-05 | All required notification persistence, serialization, v1/v2 reads, dismissal, clear, and unread counts. | Notification repository/writer, serializer tests, worker/schema/differential coverage. | A locally; complete client proof remains open |
| REST-06 | Existing and new local Paperclip media, v1/v2 media CRUD, metadata, and descriptions. | `src/paperclip.rs`, media handlers; differential `local_paperclip_media` and media write cases; `mise run cutover-integration` reopens a fresh Rustodon upload through Mastodon. | A locally; production rollback proof remains open |

### Federation

| ID | Requirement | Implementation and proof | Status |
| --- | --- | --- | --- |
| FED-01 | WebFinger, host-meta, NodeInfo, actors, Notes, and collection representations. | Federation routes in `src/web.rs`; differential `federation_discovery`; HTTP signature tests. | A locally |
| FED-02 | Signed transport, digest/skew checks, inbox enqueue, remote fetch, SSRF checks, shared-inbox deduplication, retries, and domain health. | `src/mastodon/signatures.rs`, `src/remote.rs`, `src/worker.rs`; bounded DNS answer sets, lifecycle-aware 401 classification, bounded per-client-IP signature-key refresh circuit, signed POST DNS/redirect/timeout/response-limit fixtures, signature, remote, and worker tests. | A locally; real peer proof remains open |
| FED-03 | Follow, Accept, Reject, Undo, Note, Like, Announce, Block, actor Update/Delete, audiences, replies, mentions, media, and tombstones. | ActivityPub inbox/outbox workers and restored-fixture coverage, including duplicate Accept/Reject/Block/Undo convergence, protocol-gated Accept delivery, and local-suspension actor-update reach. | A locally; complete peer/order matrix remains open |

### Durable Work

| ID | Requirement | Implementation and proof | Status |
| --- | --- | --- | --- |
| JOB-01 | Ingress, Core, Push, Pull, Mail, and Maintenance lanes. | `src/jobs.rs`, `src/worker.rs`; worker readiness and integration. | A |
| JOB-02 | Transactional enqueue, at-least-once idempotence, leases, delayed cancellation, retries, dead letters, and separate remote/media limits. | Durable queue and worker integration suite. | A locally; full production failure matrix remains open |

### Timelines and Streaming

| ID | Requirement | Implementation and proof | Status |
| --- | --- | --- | --- |
| STREAM-01 | PostgreSQL home, public, tag, and list timelines with visibility and moderation filtering. | Repository timeline projections; differential timeline and authorization cases. | A locally |
| STREAM-02 | Authenticated `user` WebSocket stream, notifications, conversations, revocation, and termination. | `src/streaming.rs`, `tests/streaming.rs`; the restored fixture proves duplicate suppression, deterministic reconnect without replay, token-specific revocation, and local-suspension `kill` termination. Browser/client acceptance and full peer convergence remain external follow-up evidence. | A locally; client/peer evidence remains open |

### Safety and Basic Moderation

| ID | Requirement | Implementation and proof | Status |
| --- | --- | --- | --- |
| SAFE-01 | Enforce account, user-domain, global-domain, suspended/silenced/deleted state, and role permissions. | `src/mastodon/policy.rs`, moderation writers, REST/federation call sites, policy schema coverage, and differential `status_authorization_matrix`. | A locally |
| SAFE-02 | Support report submission. | `POST /api/v1/reports`, report writer and notification delivery; differential/schema coverage. | A |
| SAFE-03 | Provide report, suspension, domain, status-delete, and reconciliation administration commands. | `src/main.rs`; CLI help and restored-fixture moderation coverage. | A |
| SAFE-04 | Preserve audit/history and soft-delete/tombstone/federation effects. | Write repository and ActivityPub workers; differential and rollback tests. | A locally |
| SAFE-05 | Retain login, federation, and remote-fetch abuse limits. | Login/OAuth, `media_proxy`, authenticated media-upload, ActivityPub inbox, and remote-account-resolution fixed windows use shared PostgreSQL state with namespaced keys; bounded 30-second request-body reads, route-level authorized-fetch checks, and a five-minute per-client-IP signature-key refresh circuit also exist. The two-in-flight-per-canonical-host remote-fetch budget coordinates independent web/worker pools through expiring operational leases with process-local fallback; external delivery proof remains open. | A locally; external delivery proof remains open |

### Read and Preserve

| ID | Requirement | Implementation and proof | Status |
| --- | --- | --- | --- |
| PRESERVE-01 | Tolerate historical polls/votes and reject active unsupported local polls at cutover. | Poll readers/serializers and preflight diagnostics. | A |
| PRESERVE-02 | Preserve existing lists, pins, featured/followed tags, and memberships. | Read projections and route serializers. | A locally |
| PRESERVE-03 | Preserve and apply existing filters. | Filter reads, status annotations, and route coverage. | A locally |
| PRESERVE-04 | Preserve scheduled statuses when none are pending. | Preflight scheduled-status check. | A |
| PRESERVE-05 | Preserve preview cards and custom emoji. | Repository serializers and filesystem serving. | A locally |
| PRESERVE-06 | Preserve announcements, reports, warnings, appeals, severance, migrations, imports, backups, and Web Push rows. | The deterministic fixture seeds representative rows, `verify.sql` and `verify.rb` assert their values and foreign-key links, and schema/differential/cutover snapshots prove they survive Rustodon startup and Mastodon reopen. | A locally |
| PRESERVE-07 | Read and serialize quote, collection, collection-item, and keypair rows without creating those workflows. | REST projections and serializers; route inventory. | A locally |
| PRESERVE-08 | Preserve unknown notification types and future enum values. | Open enum wrappers and notification serializer tests. | A |
| PRESERVE-09 | Leave unsupported Mastodon-owned tables untouched. | Differential database snapshots and operational-schema integration. | A locally |

## Invariant Traceability

| ID | Invariant | Implementation and proof | Status |
| --- | --- | --- | --- |
| INV-01 | Status creation updates URI, conversation, mentions, tags, media order, counters, notifications, and durable distribution. | `WriteRepository::create_status`; status write differential and worker cases. | A locally |
| INV-02 | Status deletion is soft discard plus boost/counter/conversation/tombstone work. | `WriteRepository::delete_status`; status deletion differential and worker cases. | A locally |
| INV-03 | Follow updates both counters and remote follows remain pending until Accept. | Relationship writer and ActivityPub relationship worker tests, including exact URI-bound Accept/Reject decisions. | A locally; peer ordering remains open |
| INV-04 | Favourite and bookmark target the original status, not a boost wrapper. | Interaction writer and differential interaction cases. | A |
| INV-05 | Boost creation is serialized because Mastodon has no uniqueness constraint. | Advisory locking in the writer and concurrent interaction case. | A locally |
| INV-06 | Direct status access uses mentions and optimistic conversation-array locking. | Conversation/status writer and conversation tests. | A locally |
| INV-07 | Removed mentions preserve previously granted access through silent mentions when required. | Status update mention handling and differential status-write coverage. | A locally |
| INV-08 | Poll tallies and conversation arrays use optimistic locking. | Poll/conversation repository contracts and schema tests. | A locally |
| INV-09 | Notification class and semantic type strings remain stable contracts. | Notification serializers and restored-fixture notification matrix. | A |
| INV-10 | Local accounts, login-capable users, and the instance actor remain distinct. | Account readers, authentication policy, and fixture identity checks. | A |
| INV-11 | NULL, empty strings, and empty PostgreSQL arrays remain distinct. | REST serializers, repository projections, and schema/differential cases. | A |

## Top-Level V1 Acceptance

| ID | Acceptance criterion | Required proof | Status |
| --- | --- | --- | --- |
| ACCEPT-01 | Preflight passes without user-data transformation. | `mise run preflight-integration` plus production smoke run. | M |
| ACCEPT-02 | Stop Mastodon, drain Sidekiq, start Rustodon, and retain database/media/domain/secrets. | Rehearsed fixture cutover using [`cutover.md`](cutover.md); the local rehearsal passes, while a production-window run remains required. | M |
| ACCEPT-03 | Existing users log in with password/TOTP and existing OAuth clients remain authorized. | Guarded browser authentication and 2FA-management cases plus differential OAuth cases; live browser recording remains open. | M |
| ACCEPT-04 | Pinned web frontend and recorded mobile client publish and read normal v1 content. | Browser recording plus a versioned mobile-client recording. | O |
| ACCEPT-05 | Public/private/direct content remains visible only to correct viewers. | Differential `status_authorization_matrix`, policy unit tests, REST/timeline/schema coverage, and streaming suppression coverage. | A locally |
| ACCEPT-06 | A pinned Mastodon 4.6.5 peer discovers, follows, receives, replies, likes, boosts, updates, and deletes in both directions. | `prove-mastodon-peer-federation-compatibility` fixture and peer run. | O |
| ACCEPT-07 | Worker crashes and duplicate deliveries do not duplicate effects. | Restored worker integration now covers all-lane abort recovery, database failure before acknowledgement, a twenty-job bounded-concurrency burst, duplicate relationship activities, delivery replay, live-lease Follow/Undo and Block/Undo ordering cases, and ambiguous remote-media metadata commits before and after PostgreSQL commit; peer-side idempotency, true sustained load, and hard-power-loss proof remain open. | M |
| ACCEPT-08 | Existing media works and newly uploaded images reopen after Mastodon rollback. | Media differential plus `mise run cutover-integration`, which uploads through Rustodon and verifies the row and original/small files through reopened Mastodon. | M |
| ACCEPT-09 | Unsupported active configurations fail preflight. | Preflight rejection matrix and integration task. | A |
| ACCEPT-10 | Redis can be removed and Mastodon can be restored without data migration reversal. | `mise run cutover-integration` removes the old Redis container and restores pinned Mastodon with a fresh empty Redis instance while preserving the database/media baseline. | M |

## Current Exit Condition

The local implementation and fixture gates are strong enough to continue
implementation, but v1 is not yet complete. The matrix cannot be closed until
the `O` rows are either implemented and proven or explicitly removed from the
authoritative v1 scope. In particular, live peer, browser/mobile, and
cutover/rollback evidence are still required.
