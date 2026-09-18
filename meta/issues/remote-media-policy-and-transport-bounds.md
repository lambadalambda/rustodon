# Define remote media MIME policy and scoped transport bounds

## Current status — complete (2026-09-18)

Implemented in `be6c7cd`; transport review 2 accepted strict family caps and
preserved guards. Transport 3/3 covers fixed/chunked limit−1/exact/+1, unchanged
ordinary caps, redirect policy, encoding, timeout and cancellation; formats 4/4
and configured limits 1/1 passed. Policy/transport acceptance is complete.

Parent confirms independent review 2 and approves archival after `fe6e461`.
This current status overrides historical “open”, “uncommitted”, “review pending”
and leave-uncommitted handoff instructions below. Evidence references and
slice-specific boundaries are preserved. No gates were rerun for this docs-only
reconciliation. Actual transport-loss/power-loss simulation, full fixture/release
matrices and peer gates remain unclaimed.

## Summary

First topical slice of [remote rich media](cache-remote-rich-media-previews.md),
starting from clean `bed710f`. Prepare policy and transport APIs only.

## Requirements

- Central `media_format` family/MIME policy, normalized advertised/fetched agreement,
  and explicit missing-advertisement compatibility.
- Typed media-only capability requires less than 99 MiB for audio/video and
  less than 16 MiB for images; ordinary and emoji transport caps/allowlists remain unchanged.
- Select response-family bounds before streaming. Preserve all existing network,
  encoding, timeout, concurrency and test endpoint security boundaries.
- No worker activation, serializers, schema, grants, jobs, lanes, caches or protocols.

## Acceptance Criteria

- TDD pure MIME-family tests and deterministic response/chunked/oversize transport
  tests, including unchanged ordinary limits, pass with focused bounded commands.
- Independent parent review; leave source and documentation uncommitted.

## Notes

- Created/indexed before implementation. Parent feature remains open; this slice
  makes no remote caching or preview feature claim.

## Implementation / handoff

- `RemoteMediaPolicy` uses existing `media_format` contracts and MIME list;
  case/parameters normalize, distinct MIME essences (even same-family) reject.
  `None` advertisement explicitly permits any supported fetched MIME, never a
  missing/unknown fetched MIME. Media limits are strictly less than 16/99 MiB,
  matching the processor input policy: the inclusive transport budget is the
  format input limit minus one. Ordinary transport remains inclusive. Byte probing
  is still required by the future worker.
- `RemoteMediaFetcher::new(&fetcher, advertisement)` is a typed opt-in. Its private
  inner fetcher cannot escape to generic GET/POST callers. Only this capability
  substitutes format input bounds for the ordinary response cap, before body
  streaming. Existing timeout, redirect, policy-per-hop and shared host budget
  paths are reused. Ordinary `RemoteFetchLimits::bounded` and emoji list untouched.
- No activation, persistence or serializer work. Parent feature remains open.
- Parent review `9f9122` identified the exact-limit acceptance blocker; corrected
  below. Parent review **2 pending**. Nested delegation is unavailable in this
  session. Leave uncommitted.

## Verification (2026-09-18)

Local `mise exec -- cargo test --locked --test media_formats remote_media_policy
-- --exact` could not compile existing Linux-only `paperclip` APIs on macOS (not
an implementation red). Pure test harness under ignored
`target/remote-media-slice/mime_tests.rs` imports actual `src/media.rs` and the
integration tests with crate path rewritten. `mise exec -- rustc --edition=2024
--test target/remote-media-slice/mime_tests.rs -o target/remote-media-slice/mime_tests`
failed first on missing `RemoteMediaPolicy`; after implementation its executable
passed **4/4**.

Linux source: tracked `git archive bed710f`, then only intended source/test files,
under `/srv/workspaces/rustodon-remote-policy-bed710f-alice`. NAS IPv4 resolved to
192.168.1.186; used existing `HostKeyAlias=podman-worker.local`, no host-key bypass.
Initial hostname/documented old IP attempts timed out. Image
`7203e0222e2bb72e0b83ab623051873604874cc41a688e318b84691bfa77ad8a`.
All commands sequential, `podman run --rm --name rustodon-remote-policy-tests
--cpus=4 --memory=6g --pids-limit=512 --network=none`, source mounted at `/work`,
working directory `/work`; dependency registry mounted **read-only** from
`/srv/workspaces/rustodon-null-route-20260918/cargo-home/registry` at
`/usr/local/cargo/registry`. Build output task-owned, no DB/media/production access.
Initial empty/older offline registries failed dependency resolution only.

Exact Cargo commands inside that container:

- Red: `cargo test --offline --locked --all-features --lib
  remote_media_transport_bounds -- --test-threads=1` on base plus new transport
  test: expected missing `RemoteMediaFetcher` compile failure (300s wall bound).
- Initial green (superseded by review fix below): same command on implementation:
  **1/1**, initial strengthened test **0.45s**.
  Tests real HTTP fixed/chunked bodies, >16 MiB audio/video, exact 16/99 MiB
  chunked acceptance (incorrect media semantics, fixed below), +1 rejection,
  unchanged generic 16 MiB, absent advertisement,
  normalized MIME, missing/unknown response MIME, MIME mismatch before size check.
  Listener binds ephemeral loopback; async server aborted/joined per case;
  requests 5s and entire test 30s bounded.
- `cargo test --offline --locked --test media_formats`: **4/4**.
- `cargo test --offline --locked --all-features --lib
  configured_fetch_limits_are_bounded -- --test-threads=1`: **1/1**.
- Same lib command with selectors `remote_urls_require`, `remote_address_policy`,
  `remote_dns_answer_sets`, `content_type_matching`: **1/1 each**;
  `remote_domain_budget`: **2 passed, 1 existing database test ignored**.
- `cargo clippy --offline --locked --all-features --lib --test media_formats --
  -D warnings`: **pass** (green/test+lint groups bounded 180–240s).
- Broader `cargo clippy --offline --locked --all-features --lib --tests --
  -D warnings` found two new fixture lints, fixed by helper extraction/explicit
  read-result handling, plus existing `paperclip`, worker tests, media_processor
  and mastodon_schema lints. Rerun with only those existing lint classes allowed
  (`-A clippy::needless_pass_by_value -A clippy::items_after_statements
  -A clippy::case_sensitive_file_extension_comparisons -A clippy::too_many_lines`)
  **passed**, final security-test/lint group wall bound 90s. Not a clean broad
  strict-Clippy claim.
- Local `mise exec -- cargo fmt --check`, `git diff --check`: **pass**.

No full ordinary/release, real-codec, DB, browser or peer lanes run or claimed.
Containers auto-removed; task source/build workspace retained for parent review.

## Review 1 correction — blocker `9f9122`

- Media-only response budget now uses `media_format(...).input_size_limit - 1`.
  This preserves ordinary inclusive byte limits and rejects processor-ineligible
  exact-limit media before reading fixed-length bodies, or during chunked reads.
  No new cap constants or worker changes.
- TDD red: strengthened fixed/chunked boundary matrix failed on
  `image/png length=16777216 chunked=false` against the inclusive implementation.
  Command: `cargo test --offline --locked --all-features --lib
  remote_media_transport_bounds -- --test-threads=1`, 120s container wall bound.
- Green: `cargo test --offline --locked --all-features --lib
  remote_media_transport_ -- --test-threads=1`: **3/3, 1.77s**. For image, video
  and audio, fixed and chunked tests accept limit−1 and reject exact/+1; ordinary
  fixed/chunked exact 16 MiB still accepts and +1 rejects. Direct typed-wrapper
  checks also pass redirect-hop policy denial, encoded-response rejection,
  request timeout and cancellation releasing the shared ordinary host budget.
- Rerun `cargo test --offline --locked --test media_formats`: **4/4**;
  `cargo test --offline --locked --all-features --lib
  configured_fetch_limits_are_bounded -- --test-threads=1`: **1/1**;
  `cargo clippy --offline --locked --all-features --lib --test media_formats --
  -D warnings`: **pass**. Green group wall bound 150s.
- Test-source Clippy rerun with the same four previously documented unrelated
  lint classes allowed: **pass**, 90s wall bound. No broad suite executed.
- Same task-owned NAS workspace, image, read-only registry, no-network and
  4 CPU/6 GiB/512 PID limits as above. Containers auto-removed. Local
  `mise exec -- cargo fmt --check` and `git diff --check`: **pass**.
- All changes remain uncommitted, awaiting parent review 2. Scope remains policy
  and transport only; no feature/worker/fixture-milestone claim.
