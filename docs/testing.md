# Testing

Rustodon separates fast ordinary checks from restored-database, differential,
browser, cutover, and real-peer fixtures. Each lane has distinct prerequisites
and proves a distinct compatibility boundary; source presence or a passing
lighter gate must not be used as a substitute.

## Ordinary checks

Mise pins the toolchain and exposes the standard aggregate:

```console
mise install
mise run check
```

`check` runs:

- formatting and strict Clippy;
- ordinary default-feature, all-feature debug, and all-feature release tests;
- dependency advisory, license, ban, and source policy;
- static fixture verification;
- vendored worker-media verification;
- offline shell and Python harness regressions.

Ignored database/container/browser/peer tests are excluded. Do not replace named
integration gates with blanket `cargo test -- --ignored`: ignored tests require
different roles, environment, source, media, and lifecycle setup.

Individual ordinary tasks are also available:

```console
mise run fmt
mise run lint
mise run test
mise run deny
mise run fixture-verify
mise run worker-media-verify
mise run harness-tests
```

## Integration gate map

| Lane | Command | Contract |
| --- | --- | --- |
| Pinned source | `mise run pinned-source-contracts` | Independently derives frontend and Rails contracts from the exact source revision. |
| Media processor | `mise run media-processor` | Runs pinned tiny media through the real bounded FFmpeg/ffprobe paths and verifies the complete advertised capability set. |
| Mastodon schema/HTTP | `mise run mastodon-schema-integration` | Restores the fixture and runs named read/write/protocol selectors with least-privilege roles. |
| Operational schema | `mise run operational-schema-integration` | Creates/upgrades Rustodon-owned tables, rejects drift, and verifies Mastodon data remains unchanged. |
| Standalone bootstrap | `mise run standalone-bootstrap-integration` | Initializes an empty PostgreSQL 14 database and runs bounded login/media/status/discovery smoke without Mastodon. |
| Startup | `mise run startup-integration` | Proves web and worker processes fail closed on unsafe configuration or privileges. |
| Preflight | `mise run preflight-integration` | Exercises canonical and rejection configurations against disposable clones. |
| Workers | `mise run worker-integration` | Exercises durable queue semantics, all lanes, recovery, readiness, and shutdown. |
| Required differential | `mise run differential-ci` | Runs the bounded high-value Rails-versus-Rust compatibility set with fresh fixtures. |
| Full differential | `mise run differential-full` | Runs every registered differential case plus both media-root representations. |
| Cutover | `mise run cutover-integration` | Rehearses migration, Rustodon smoke, rollback, Mastodon reopen, and preservation checks. |
| Browser | `mise run browser-integration` | Drives the pinned frontend in Chromium within the HTTPS cutover fixture. |
| Real peer | `mise run peer-public`, `peer-privacy`, `peer-notes`, `peer-profile`, `peer-interactions` | Runs explicit manual scenarios against a disposable pinned Mastodon peer. |

The `media-processor` lane requires the production `ffmpeg` and `ffprobe`
executables on `PATH`; it is the named real-codec gate and runs serially. The
ordinary suite still covers deterministic MIME, probe-limit, subprocess I/O,
timeout, cancellation, and child-reaping behavior without requiring host
codecs.

The poll-expiration portion of worker startup proves an immutable activation
boundary plus one bounded reconciliation segment, not completion of the full
historical scan. Remaining work must already be represented by a committed
Maintenance continuation before readiness; independently guarded Core handlers
baseline historical generations when that continuation is processed later.

The workflow definitions in [`.github/workflows/ci.yml`](../.github/workflows/ci.yml)
show how ordinary, source, schema, worker, differential, and extended lanes are
grouped. Workflow presence is configuration, not proof that a particular commit
or environment passed.

Run container-backed fixtures sequentially. They currently target GNU/Linux
x86-64 and require Podman, exact cached/pinned images where specified, sufficient
disk and memory, and workload-side timeouts. The standalone lane additionally
requires a local Podman engine because its random PostgreSQL port is bound only
to loopback.

## Focused local-upload HTTP slice

The opt-in test below requires a **disposable restored** PostgreSQL fixture and the
real media-tool capability set; it holds the worker until pending assertions and
then drives the production handler registry with a bounded executor. It is not a
browser test or the complete schema/worker lane:

```console
cargo test --locked --all-features --test media_state local_upload_http::local_rich_upload_http_lifecycle -- --ignored --exact --nocapture --test-threads=1
```

Three distinct URLs to the same **task-owned** database are mandatory (no owner
fallback):

- `RUSTODON_OPERATIONAL_DATABASE_URL`: owner, only for setup and assertions.
- `RUSTODON_WORKER_DATABASE_URL`: narrow runtime, used by the HTTP repository,
  shared rate limiter, queue/outbox dispatcher, and worker heartbeat/readiness.
- `RUSTODON_WORKER_WRITE_DATABASE_URL`: narrow writer, used by HTTP mutations and
  the actual registered worker handlers.

On a fresh restored fixture, the ignored `local_upload_http::local_rich_upload_schema_setup`
selector performs owner-only operational migration before grants. Provision dedicated
non-superuser/non-owner roles with no memberships using the existing install contract:
unchanged `docs/mastodon-refresh-instances.sql`, `docs/mastodon-writer-grants.sql`, and
the runtime grant profile in `src/bootstrap.rs::apply_runtime_grants` (also represented
in the worker fixture runner). Do not add grants to make a failing test pass. The
lifecycle setup supplies the actual writer name to the existing migration validator.
It asserts distinct restricted logins and verifies representative forbidden SQL
operations really return PostgreSQL `42501`.

The separate focused readiness test starts the real worker loop with these same
restricted pools and checks generic/missing-root/wrong-lane/fully configured handler
capability, periodic heartbeat refresh, and removal on shutdown:

```console
cargo test --locked --all-features --test media_state local_upload_http::local_rich_upload_restricted_worker_readiness -- --ignored --exact --nocapture --test-threads=1
```

The focused native-media authorization regression uses the same restricted roles and
setup, with persisted browser sessions/backing OAuth tokens and the existing JPEG
state fixture (including stale pending/failed bytes):

```console
cargo test --locked --all-features --test media_state local_upload_http::local_upload_browser_media_access -- --ignored --exact --nocapture --test-threads=1
```

It checks owner-only unattached reads, attached visibility, bearer precedence,
session lifecycle, immutable session/token snapshots, conditional/range ordering
and private success/denial caching. Its dangling-reference setup is owner-only in
the disposable database; HTTP still uses the runtime/writer pools. This is not a
browser run. Unsatisfiable ranges retain the existing Rack/Rails final 404.

Use a fresh restore per invocation: the HTTP test exercises the real shared 30-upload
rate limit without bypass/reset, and readiness starts the existing maintenance startup
reconciliation. These tests are not the full CLI/startup fixture lane. The default
`media_state` schema selector still runs its existing non-codec matrix without
`test-support`; it does not imply these opt-in tests ran. DEVLOG records exact bounded
NAS role provisioning, execution, and source hashes.

Local v2 modern stills/video/audio return 202, then owner polling returns 206 until
ready (200) or retained failure (422). Modern stills are deliberately asynchronous,
unlike the pinned synchronous modern-still controller path. Browser completion is
not established by these HTTP results. For writable installations, `admin
worker-readiness` also requires a fresh local-upload-capable Maintenance heartbeat;
`/ready` retains its existing database-only meaning.

## Focused instance-activity storage/auth slice

Migration 6 adds exact daily memberships and per-bucket sliding expiry. The new
runtime grants are SELECT-only; writer grants remain separate. Use a fresh,
disposable PG14 restore with the owner/runtime/writer URLs and documented grant
profiles described above. Run these selectors serially, with explicit resource
and wall-time bounds:

```console
cargo test --locked --all-features --lib activity:: -- --ignored --nocapture --test-threads=1
cargo test --locked --all-features --test operational_schema instance_activity_upgrade_from_five_preserves_history_and_grants -- --ignored --exact --nocapture --test-threads=1
cargo test --locked --all-features --test media_state local_upload_http::local_upload_browser_media_access -- --ignored --exact --nocapture --test-threads=1
```

The upgrade selector deliberately drops/recreates the two activity tables and
version-6 ledger entry in its task-owned fixture; never point it at an instance.
The media selector reuses persisted GET/HEAD cookie/bearer cases, snapshots
activity/sign-in state with a due owner, then tests explicit credentials and
frontend HTML tracking. This is HTTP evidence, not an executable-browser gate.
Only these interactive routes and existing browser settings/session hooks track
retained activity; this is not full Rails controller or generic bearer tracking.

Fresh bootstrap's existing selector now verifies the owner's initial activation,
unchanged expiry on verification rerun, and invalid baseline activity rejection.
The pinned source lane includes `pinned_daily_activity_records_and_interactive_tracking_contract`.
Neither executable test source nor this documentation claims those gates ran;
DEVLOG records exact bounded runs and environment/tool limitations.

No historical sign-ins are backfilled. Retention is **15,778,476 seconds** reset
on every eligible bucket write, not 24 weeks. Cached aggregation/public values
and physical maintenance pruning remain a subsequent slice: readers must exclude
expired buckets and today from the preceding 28/168-day unions. Cleanup must
lock each bucket and remove members and bucket together, preserving the
transactional relationship without a public user FK or historical-state filtering.

## Standalone bootstrap

`mise run standalone-bootstrap-integration` starts from an empty PostgreSQL 14
database and an empty local-media root. It uses a pinned PostgreSQL child image
and task-owned database, roles, volume, network, random loopback port, and marked
media directory with explicit CPU, memory, process, readiness, and wall-time
bounds. It executes one exact ignored test serially and removes only resources
created by that invocation.

The lane does not obtain Mastodon source, restore the populated compatibility
fixture, or run a Mastodon image, Rails, Sidekiq, or Redis. It covers fresh
installation, exact verification rerun, schema/role/grant/data/media rejection
cases, first-Owner browser login, media upload, public status creation,
WebFinger, and ActivityPub. See the [standalone setup guide](standalone.md) for
the operator path and its backup boundary.

A cold run may pull the immutable PostgreSQL image after verifying that the
pinned index contains the expected `linux/amd64` child. Run the heavy lane
sequentially:

```console
mise run standalone-bootstrap-integration
```

`mise run harness-tests` exercises the lane's selector, environment isolation,
resource naming, and success/failure cleanup with offline stubs. That wiring
check is not evidence that PostgreSQL bootstrap or the HTTP smoke passed.

## Compatibility fixture

The fixture target is Mastodon v4.6.5 commit
`1440d55b139e39ec722c2a3db7f60b66cd889048`, schema version
`20260611150940`. Release-versioned database, catalog, migration, and media
artifacts live under
[`fixtures/mastodon/v4.6.5/`](../fixtures/mastodon/v4.6.5/).

```console
mise run fixture-obtain
mise run fixture-generate
mise run fixture-verify
mise run fixture-restore-verify
mise run fixture-repro
```

`fixture-obtain` places the exact source under ignored `target/`.
`fixture-generate` rejects a dirty or wrong-revision checkout and uses immutable
Mastodon/PostgreSQL child manifests. It uses dedicated `.invalid` domains and a
dedicated disposable database, never project or Mastodon environment files.
Redis is not required for generation.

`fixture-restore-verify` validates the checked dump through SQL and Mastodon
Rails, including notification variants and Paperclip media. `fixture-repro`
regenerates all artifacts and compares them recursively byte for byte.

Fixture identities, key/media provenance, normalization, and release-update
instructions are in the [fixture README](../fixtures/mastodon/v4.6.5/README.md).
Vendored worker media has its own manifest and verification command; an image
extract is not a substitute for the full source-contract lane. Quote contracts
inspect the exact compose, controller/service, ActivityPub lifecycle, counter,
and serializer files at the pinned revision. A source checkout is configuration,
not a pass: record compile or platform failures separately from executed contract
results.

## Differential testing

The differential harness starts pinned Mastodon and Rustodon against independent
database and media clones, sends the same HTTP request to both, and compares:

- status and declared headers;
- canonical JSON and exact scalar/null/array distinctions;
- selected logical database rows;
- media contents and metadata before and after writes.

Normalization is narrow and validates nondeterministic request IDs, generated
timestamps, or prefixed random tokens before replacing them. The harness does
not compare Rails callbacks, SQL ordering, Redis keys, or Sidekiq payload
representation.

Run one case by its Rust test name:

```console
mise run differential -- oauth_bearer_authentication
mise run differential -- status_authorization_matrix
mise run differential -- rest_protocol_contracts
mise run differential -- federation_discovery
mise run differential -- poll_lifecycle
mise run differential -- quote_lifecycle
mise run differential -- local_web_client_shell
```

Each isolated invocation gets fresh marked databases, media roots, Redis state,
and ports. Production-looking URLs and unmarked paths are rejected. Quote-specific
schema/privilege and worker selectors are also available without broadening their
heavy lanes:

```console
tools/mastodon-fixture schema-read-test quote_lifecycle
tools/mastodon-fixture worker-test quote_federation_lifecycle
```

These selectors are executable intended evidence. The `schema-read-test` selector
checks the restored quote schema/default and narrow writer privileges; it is not a
quote lifecycle execution. The differential checks exact durable quote-notification,
QuoteRequest, author-stream, update, rollback, and persisted-state effects. The worker
selector uses the restricted writer role and checks allowed/denied embedded imports,
signed target-owner scalar dereference with same-job retry, replay, signed outbound
QuoteRequest/Accept/Reject HTTP delivery, transition-specific
acceptance/rejection/revocation streams and status updates, legacy counter stability,
remote quoting-Note deletion cleanup, conflicting replay, signed original-payload
forwarding, and block/terminal-transition suppression of reclaimed active leases.
Quote-related delivery jobs carry exact request/quote/status metadata; the
final database fence retains its row lock through the bounded HTTP attempt, while
terminal transitions cancel only safe unleased or expired work. Their registration in
the required inventory prevents quote regressions from being silently skipped, but they
do not count as passing final-tree evidence until the corresponding disposable
PostgreSQL/worker lanes have actually completed.

## Browser testing

The browser lane starts the HTTPS cutover fixture and drives the pinned frontend
through `agent-browser`. It covers anonymous and authenticated shells, React
mounting, SPA navigation, password/backup-code session setup, settings and
logout, Home boost/reply controls, leading/trailing web-settings updates,
persistence after reload, an Alice quote create → nested timeline/permalink →
reload lifecycle, and a two-account poll composer → vote → refresh lifecycle.
The quote smoke uses the pinned frontend's `Boost or quote` menu and stable quote
container selectors, captures both generated status IDs, fails closed at every
checkpoint, and deletes the quote before its target. Unexpected API failures and
the enclosing exact rollback comparison are also covered. The quote and poll
scenarios are executable intended acceptance coverage, but their final-tree
browser run (and the corresponding restored-schema and worker fixtures) is
explicitly deferred; source presence and passing offline harness contracts alone
are not acceptance evidence.

Optional evidence paths:

```console
RUSTODON_BROWSER_RECORD=browser.webm \
RUSTODON_BROWSER_SCREENSHOT=browser.png \
  mise run browser-integration
```

The fixture uses an ephemeral exact-domain certificate and scoped trust inputs;
it does not install global trust or weaken production cookies, CSRF, TLS, or
request deadlines. This smoke does not cover every form, WebSocket, or
EventSource behavior.

## Federation peer testing

Peer lanes are manual, local-engine, cache-only, and require an ignored
mode-600 workspace marker plus explicit opt-in through their Mise tasks. The
runner validates its physical repository/target layout, fixture
inventory, immutable image digests/platforms, resource absence, mount/network
visibility, selected ignored test, bounded deadline, and cleanup. It never pulls
or builds a fallback image.

```console
mkdir -p target
(umask 077; printf 'rustodon-peer-workspace-v1:%s\n' "$(pwd -P)" \
  > target/.rustodon-peer-workspace)
mise run peer-public
mise run peer-privacy
mise run peer-notes
mise run peer-profile
mise run peer-interactions
```

The scenarios cover discovery/follow/public delivery, followers-only and direct
privacy, Note create/update/delete, profile updates, Like/Undo, and private
Announce/Undo. Success requires received-state convergence and a successful
signed delivery; inbox HTTP acceptance alone is insufficient.

Test routing is available only in debug `test-support` builds. It maps exact
`.invalid` HTTPS origins to loopback while retaining hostname verification,
signing, production URL identity, and fail-closed behavior for unmapped
destinations. See [Federation peer smoke](federation-peer-smoke.md) for detailed
assertions and limitations.

The retained five-scenario Mastodon result predates later browser/API changes and
is not final-tree certification. It omits an explicit reply scenario,
simultaneous convergence stress, full notification/counter parity, production
behavior, and Pleroma interoperability.

## Pleroma status

Pleroma interoperability remains unclaimed. The preparation helper validates an
exact source archive and lockfile and can build only from already-cached,
immutable `linux/amd64` base images with `--pull=never`. Missing prerequisites
fail closed rather than using floating tags or credentials. See
[Pleroma build provenance](federation-pleroma-build.md).

## Resource and secret safety

- Use disposable databases, roles, identities, `.invalid` origins, and media
  roots; never point fixture commands at production.
- Keep source-contract, image-only, database, browser, and peer claims separate.
- Synchronize only intended tracked source and explicit new fixtures. Do not copy
  `.git`, `target/`, environment files, credentials, backups, or unrelated
  untracked files.
- Bound CPU, memory, process count, and wall time. Run heavy fixtures one at a
  time.
- Clean only resources created by the current run. Never broadly prune an engine
  shared with unrelated workloads.
- Preserve command exit status and treat timeouts or interrupted cleanup as
  failures until task-owned resources are inspected.

## Focused instance activity aggregation slice

After restoring/migrating a disposable PostgreSQL 14 fixture and applying the
existing restricted runtime and writer grant profiles, run these serially:

```console
cargo test --locked --all-features --lib activity::aggregation_tests::
cargo test --locked --all-features --lib activity::tests:: -- --ignored --test-threads=1
cargo test --locked --all-features --lib worker::activity_tests:: -- --ignored --test-threads=1
```

`tools/mastodon-fixture worker-test instance_activity` runs the two ignored
selectors on a fresh restored fixture with the correct roles; the full worker
lane includes them.

These use `RUSTODON_OPERATIONAL_DATABASE_URL` (owner),
`RUSTODON_WORKER_DATABASE_URL` (restricted runtime), and
`RUSTODON_WORKER_WRITE_DATABASE_URL` (restricted writer). Activity tests clear only
fixture activity tables and create synthetic accounts; **restore a clean fixture
before the main-process test** because these accounts deliberately lack signing
keys and must fail production startup validation.

```console
cargo test --locked --all-features --test startup \
  main_runtime_activity_counts_cache_privacy_and_initial_metadata \
  -- --ignored --exact --test-threads=1
```

Main-process testing uses the existing `RUSTODON_STARTUP_DATABASE_URL`,
`RUSTODON_STARTUP_WRITE_DATABASE_URL`, `RUSTODON_STARTUP_OWNER_DATABASE_URL`, and
`RUSTODON_STARTUP_MEDIA_ROOT` fixture variables. It launches actual web binaries,
checks both federation modes, initial HTML metadata, cache sharing with activity
tables exclusively locked, and zero-count fallback with an empty cache. This
is not a browser/sidebar rendering gate or the complete startup lane. Worker
coverage invokes the registered maintenance handler with restricted pools; it is
not a claim of the complete durable worker lifecycle lane.
