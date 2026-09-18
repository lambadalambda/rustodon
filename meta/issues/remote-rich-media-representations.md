# Consistent remote rich-media REST, ActivityPub, and proxy representations

## Current status — complete (2026-09-18)

Implemented in `8b2f49e`; representation review 2 accepted library 351 passed /
18 ignored, REST 18/18, pinned source 8/8 and restricted private-byte HTTP 1/1.
Actual PNG/JPEG, thumbnail identity, GET/HEAD/range, readiness denial, private
authorization/cache behavior and REST/AP URL/MIME agreement complete this slice.
Opaque rich HTTP fixtures are not playback proof; worker/browser evidence supplies
that separately. Full cookie/session lifecycle integration was not rerun.

Parent confirms independent review 2 and approves archival after `fe6e461`.
This current status overrides historical “open”, “uncommitted”, “review pending”
and leave-uncommitted handoff instructions below. Evidence references and
slice-specific boundaries are preserved. No gates were rerun for this docs-only
reconciliation. Actual transport-loss/power-loss simulation, full fixture/release
matrices and peer gates remain unclaimed.

## Summary

Subissue of [Cache remote rich media with previews](cache-remote-rich-media-previews.md), based on worker normalization at `41a771e`.

## Requirements

- Pending/failed rich previews must never fall back to original video/audio bytes; audio previews are null unless a supported image thumbnail exists.
- Cached original/small URLs and MIME must identify the same authorized local representation, including MP4/PNG and normalized HEIC/JPEG. ActivityPub uses generated small without a duplicate thumbnail scheme.
- Cached responses support existing rich-family bounds without widening ordinary limits; preserve authorization, SSRF, cookie/bearer precedence, private caching, ranges and HEAD.
- No synchronous transcoding, remote browser hotlinks, new fields/protocol metadata, or remote-thumbnail jobs. Ordinary pending-image fallback remains compatible with image MIME checks.

## Acceptance Criteria

- Pure REST/AP regression tests and actual authorized cached-byte HTTP tests cover PNG validity, null audio, JPEG URLs, URL/MIME agreement, fail-closed rich small, and unauthorized denial.
- Focused tests use disposable restricted roles on bounded NAS tools7203/PostgreSQL 14 fixtures, never production. Run pinned source contracts if expected pinned wire semantics change.
- Independent review (at most two substantive rounds); leave changes uncommitted.

## Notes

Browser implementation and evidence belong to the next slice. Record exact executed gates and remaining evidence boundaries here.

## Implementation and evidence (2026-09-18)

Implemented on `41a771e`; **uncommitted, pending independent parent review**.

- REST suppresses unavailable pending/failed remote rich previews, retains null
  audio previews without a supported image thumbnail, and uses existing cached
  original/small paths. After review 1, explicit thumbnail cache namespaces
  follow the attachment model's locality (original `remote_url`), matching the
  pinned source and authorization repository.
- ActivityPub prefers installed local originals over historical remote source
  URLs. Generated small icons use the existing style MIME/path contract; audio
  has no generated icon. Explicit thumbnails require supported image MIME.
- Proxy small misses never select rich originals. Installed cache misses do not
  fall back to potentially differently encoded remote sources. Pending/failed
  cached bytes are not served. Raster fallback checks fetched MIME and image
  signature, and original-image fallback also requires advertised MIME agreement.
- Cached proxy responses stream through the existing Paperclip range/HEAD and
  conditional machinery. The existing family contract permits 99 MiB rich
  originals; small/ordinary images remain capped at 16 MiB. No generic transport
  cap changed. Authorization/domain-policy checks still precede file access.
  Proxy reuses the established cookie/bearer viewer and applies private/no-store
  plus credential Vary to success and failure responses.
- No schema, job, thumbnail duplication, worker, synchronous processor, browser,
  SSRF transport, or production resource changes.

### Executed gates

NAS workspace `/srv/workspaces/rustodon-representations-41a771e-alice`; logs and
exact runner/setup commands in `evidence/`. Tools image
`7203e0222e2bb72e0b83ab623051873604874cc41a688e318b84691bfa77ad8a`;
PG14 image `1a6c2409ab71f4d054d676ba09d9b74b5d843d805bd5a0cf08314d27ca659d37`.
Sequential test containers: 4 CPU, 6 GiB, 512 PIDs, 870s container/900s outer
wall bounds; database 2 CPU, 1 GiB tmpfs, 128 PIDs, 1800s lifetime. Ordinary
checks ran without network. Reused the prior worker task's build cache, not its
source or database. Source sync used intended tracked files plus the new issue;
no instance data, credentials, `.git`, or build output was copied.

- Red REST selector: **0/1**, rich pending preview incorrectly non-null
  (`red-rest.log`). Red AP selector: **0/1**, source URL used with normalized MIME
  (`red-ap.log`). Red proxy helper group: **7 passed, 2 failed, 1 ignored**,
  stale pending bytes and rich 16 MiB ceiling (`red-proxy.log`).
- `cargo test --offline --locked --all-features --test rest_serializers --
  --test-threads=1`: **18/18** (`final-rest.log`).
- `cargo test --offline --locked --all-features --lib -- --test-threads=1`:
  **350 passed, 18 ignored**, 7.36s (`final-lib.log`), including 27 AP tests and
  11 ordinary cached/proxy regression tests.
- `cargo test --offline --locked --all-features --lib
  cached_private_media_http_requires_status_access_even_when_files_exist --
  --ignored --nocapture --test-threads=1`: **1/1**, 5.24s (`final-http.log`).
  Fresh restored PG14 fixture, dedicated restricted read-only/non-owner login
  for HTTP, owner solely setup/assertions. Test checks distinct identities,
  privilege flags and absence of attachment UPDATE permission. It proves actual
  decodable PNG/JPEG bodies, proxy and exact local-URL GET/range/HEAD, audio small
  denial, pending/failed rich small denial, and unauthorized cached-byte denial
  before/after successful reads. Rich original bytes in this test are opaque
  fixtures, not codec/playback evidence (the previous worker slice owns that).
- Verified exact clean Mastodon revision before running
  `cargo test --offline --locked --test pinned_source_contracts -- --ignored
  --test-threads=1`: **8/8** (`source-verify.log`, `source-contracts.log`). Local
  reference verification passed, but local execution failed on Linux-only
  rustix APIs. Existing NAS source copies were not Git checkouts and were
  rejected, not substituted. Obtained and verified the pinned checkout using
  `tools/mastodon-fixture obtain`; verified on NAS host because tools7203 lacks
  Git, then tested with that same checkout mounted read-only in the container.
- Strict scoped `cargo clippy --offline --locked --all-features --lib --test
  rest_serializers -- -D warnings`: **pass** (`final-clippy.log`). Broad `--tests`
  strict run exposed one new oversized AP test (split into shared fixture and
  separate normalized-representation test) plus existing unrelated lints in
  media_processor, paperclip, local_uploads tests and mastodon_schema. Broad
  rerun with only documented `case_sensitive_file_extension_comparisons`,
  `needless_pass_by_value`, `items_after_statements`, `too_many_lines` allowances:
  **pass** (`clippy-tests-known.log`); no unrelated edits.
- Local `mise exec -- cargo fmt --check` and `git diff --check`: **pass**.
  Final source hashes recorded in `evidence/final-source.sha256`. Task PG14 and
  network removed; automatic test-container cleanup, source/evidence retained.

### Remaining boundaries

- Nested subagent delegation is disabled (depth limit). Independent **parent
  review is pending**, not claimed complete; no substantive review rounds used.
- Browser preview/playback/reload is the next slice. No browser, peer, release,
  full schema/worker, or new codec gate was run here.
- Existing full cookie/session lifecycle integration was not rerun; this slice
  reuses that viewer implementation and executes the focused bearer/private
  authorization matrix. Remote raster fallback safety has pure/source-level
  checks, not a new external-upstream HTTP fixture.
- Keep this subissue and the parent open pending review and their respective
  acceptance boundaries.

## Review 1 — bounded corrections

Two relevant mediums accepted before code:

- M1: resolve explicit `MediaThumbnail` storage identity across serialization,
  authorization and cleanup. Initial hypothesis was `thumbnail_remote_url`, but
  pinned source inspection rejected it: `config/initializers/paperclip.rb:16-29`
  uses `attachment.instance.local?` for both prefixes, and
  `app/models/media_attachment.rb:231-233` derives locality from `remote_url`.
  Keep the repository's original identity and align REST/AP plus the three
  existing thumbnail metadata/cleanup constructors. Generated `MediaFile/small`
  remains unchanged.
- M2 chosen rule: direct remote `MediaFile` paths reject explicit processing
  states 0/1/3 before opening bytes. Preserve NULL and ready 2, explicit thumbnails,
  and existing local attached/unattached authorization. Apply the explicit-state
  rule to remote MediaFile regardless of MIME because normalized HEIC/JPEG no
  longer retains reliable rich-source provenance. This is not a blanket readiness
  requirement for local media or historical attached NULL rows.

Add actual restricted-role GET/HEAD/range regressions first; keep uncommitted for
parent review 2. No browser work or broad abstraction/schema changes.

### Review 1 results — ready for parent review 2

Both relevant mediums resolved without new fields, queries, schema or abstraction.
The direct readiness guard reuses already-loaded authorization facts and applies
only to remote `MediaFile` paths with explicit 0/1/3. It does not gate explicit
thumbnails or change local attached/unattached policy; NULL and 2 still authorize
when normal status access permits. The original thumbnail SQL remains unchanged.

Same bounded NAS7203/PG14 setup and read-only role as the first slice. Owner was
used only for fixture setup/reset and assertions. Review logs are in the same
workspace `evidence/`:

- Red actual serialized-thumbnail HTTP URL: **0/1**, absent thumbnail remote URL
  emitted an unservable non-cache path, GET returned 404 rather than 200
  (`review1-m1-source-red.log`). Initial hypothesis-only run is retained as
  `review1-m1-red.log`; it is superseded by pinned-source-aligned storage identity.
- Red direct remote non-ready file HTTP: **0/1**, processing=0 MP4 original GET
  returned 200 rather than 404 (`review1-m2-red.log`).
- Green `cargo test --offline --locked --all-features --lib
  cached_private_media_http_requires_status_access_even_when_files_exist --
  --ignored --nocapture --test-threads=1`: **1/1, 7.74s**
  (`review1-http-green.log`). Added checks cover actual REST-emitted thumbnail
  GET/HEAD/range with absent/present thumbnail remote URL; wrong namespace with
  stale bytes denies; generated small remains readable. Exact installed MP4,
  MP3 and normalized JPEG originals/previews deny all GET/HEAD/range requests in
  explicit states 0/1/3, permit NULL and 2, and permit transition back to 2.
  Explicit-thumbnail and historical attached-local readiness conventions remain
  readable. Existing unauthorized/private-cache checks still pass.
- `cargo test --offline --locked --all-features --lib -- --test-threads=1`:
  **351 passed, 18 ignored, 7.27s** (`review1-lib.log`), including the new pure
  thumbnail metadata identity matrix. REST serializer suite **18/18**
  (`review1-rest.log`); fixture now explicitly sets thumbnail schema version 1
  when asserting a cache namespace.
- Strict scoped Clippy **passes** (`review1-clippy.log`). Broad test Clippy with
  the same four documented pre-existing lint allowances **passes**
  (`review1-clippy-tests-known.log`); no new lint allowances in source.
- Exact clean pinned-source verification followed by source contracts **8/8**
  (`review1-source-verify.log`, `review1-source-contracts.log`). Local fmt/diff
  checks pass. Source hashes recorded as `review1-source.sha256`.

Task PG14/container/network resources removed; evidence/source retained.
One substantive correction round is complete. **Leave uncommitted for parent
review 2**, then pause; no browser work or expanded fixture-lane claim.
