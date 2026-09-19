# Focused hashtag-control acceptance

Owner: [acceptance subissue](../../meta/issues/accept-local-hashtag-controls-browser-differential.md).
Application: **bd01acae2bc4e1b8a75bd95648e216535c790330** (reviewed profile-read prerequisite).
Reference: clean, read-only Mastodon 4.6.5 **1440d55b139e39ec722c2a3db7f60b66cd889048**.
No application code changed by this acceptance work. Parent review **75f770**
approves the bounded harness commit without blockers. **Browser PASS; strict
differential FAIL (54/59)** is preserved. Equivalent statuses/rejection semantics
are accepted for ordinary compatibility; this is not bug-for-bug text parity.

## Actual browser result

Final fresh uninterrupted controller passed:

- Real login, verified pinned assets, header and nonzero seven-day current-DB history.
- Native Follow/Unfollow clicks, observed POST responses, state after reload.
- Home excludes a task status from an unfollowed author before following its tag,
  includes and renders it afterward, excludes it after unfollowing.
- Native dropdown Feature/Unfeature clicks, POST responses, reload state, and
  corresponding public-profile tag presence/removal.
- Native profile suggestion add → `POST /api/v1/featured_tags` **200**;
  item/Delete control persists after reload and tag appears on public profile.
- Native Delete click → `DELETE /api/v1/featured_tags/9204` **200**;
  reload of consolidated `GET /api/v1/profile` and public profile confirms removal.

No API mutations substituted for clicks, no injected responses, Redux writes or
profile PATCH. Public profile checks are signed-in views of the public route,
not an anonymous session. `observe.js` passively records every frontend API XHR's
method/path/status and allowlisted tag fields/home IDs; never headers, credentials,
arbitrary response bodies or raw HAR. Each stage retains actual calls/DOM and a
screenshot. Document reloads deliberately clear the observer and frontend cache.

There were three fresh resumed browser invocations: first interrupted requests
under the inherited 1-GiB web bound (cause unproved, no successful gate claimed);
second with a 2-GiB bound exposed a previously unreachable DOM-valued CDP wait;
third with boolean waits passed end-to-end. No app fixes. Earlier `8e22f52`
profile-404 evidence remains historical in its old workspace/issue notes.

## Actual Rails differential result

**Second and final full attempt: 59 comparisons, 54 strict matches, all 59 status
codes match.** No expected-response fixture substitutes for Rails. The driver
sends equivalent requests to running pinned Rails Puma and actual Rust web
processes on independent restored DB/media clones. It captures complete actual
tag/error JSON responses, status and content-type essence before comparison.

Executed: existing/public/unpersisted/case-normalized/invalid tag lookup;
Follow/Unfollow/Feature/Unfeature and idempotence; collection reads/create/delete;
whitespace/hash normalization; exact/case duplicate behavior; nonzero count/date;
public featured read; owned versus other-owner IDs and repeated deletion;
anonymous/invalid/revoked/application-only tokens; read/write-account,
write-follows, legacy follow and wrong-scope cases; ten successful creations,
eleventh collection/header rejection, existing-header idempotence at limit.
The disabled-user probe also lacks write scope: it is **not** independent proof
of suspended-account precedence. No claim of exhaustive auth or protocol parity.

Nonzero featured count **"3"**, date **2026-09-19**, final collection count **10**
matched. Narrow normalization only:

- Seven-day history is validated for numeric-string fields/consecutive days and
  retained raw, but excluded from equality: Rust current-DB aggregate is not
  Rails Redis retention/activity. Actual first-day Rails uses/accounts **0/0**,
  Rust **1/1**; no values replaced with synthetic zeros.
- Numeric IDs are paired bijectively within tag/featured tables from actual
  responses, rejecting unstable mappings; returned real IDs drive owner checks.
- Only contiguous equal-`statuses_count` collection ties are sorted by name;
  Rails declares descending count, not a tiebreaker. Other ordering and scalar
  types remain intact. Content-type charset is not compared.

Five **unhidden error-string differences** keep the strict result FAIL:

| Case | HTTP (both) | Rails | Rust |
| --- | --- | --- | --- |
| Invalid lookup (`!!!`) | 404 | `Not Found` | `Record not found` |
| Missing / empty collection name (2 cases) | 400 | `param is missing or the value is empty or invalid: name` | same except missing `or invalid` |
| Invalid collection name (`!!!`) | 422 | `Validation failed: Name can't be blank, Name is invalid, Display name is invalid` | `Validation failed: Name is invalid` |
| Eleventh header feature | 422 | maximum-hashtags message repeated three times | one maximum-hashtags message |

No app fix or error-message normalization was made. Parent review **75f770**
accepts these five differences as nonblocking ordinary compatibility unless an
actual client dependency demonstrates otherwise. All 59 statuses and equivalent
rejection semantics match. Do not reproduce incidental duplicate upstream wording
or convert the retained strict FAIL into a fabricated PASS.

First actual attempt also completed 59 requests, with nine strict mismatches.
Four were a fixture artifact: rewriting an older-ID status date made Rails'
ID-ordered `pick(:created_at)` differ from max(timestamp). Second attempt instead
inserts a new highest-ID current status, preserving chronological fixture order.
All count/date responses then matched. No application timestamp change, race
reproduction or third differential attempt. First-attempt evidence stays separate.

## Reuse, isolation and bounds

`prepare.py` emits a fixed task-only adaptation of `tools/remote-browser` for
`browser` (default) or `differential`. Build, archive provenance, PG14 restore,
migration/narrow grants, TLS/browser wrapper and teardown are reused. Differential
extracts the existing pinned image/constants and `start_differential_web` function
from `tools/mastodon-fixture`: adds resource bounds/`--pull=never`, removes host
publication, fixes task name, disables unrelated feed preparation. Reference
checkout is never modified/mounted writable. No general orchestration framework.

Both modes use separate fixed NAS workspaces and run serially:

- `/srv/workspaces/rustodon-hashtag-browser-bd01aca-alice`
- `/srv/workspaces/rustodon-hashtag-differential-bd01aca-alice`

Fresh source came only from `git archive bd01aca`; harness sync was tracked
`tools/remote-browser`, `tools/mastodon-fixture` and explicitly named new helpers.
No instance env, credentials, backups, `.git`, unrelated files or existing build
output were copied. Existing dependency cache reused, task-owned fresh output.
Do not rerun over existing evidence; choose/review a fresh fixed namespace first.

```sh
PYTHONDONTWRITEBYTECODE=1 python3 tools/tests/hashtag-controls-browser-test.py
tools/mastodon-fixture verify-source target/mastodon-v4.6.5
# In the correctly named fresh NAS workspace after archive + explicit helper sync:
python3 harness/tools/hashtag-controls-browser/prepare.py harness harness/tools/hashtag-runtime browser
# Or: ... differential
# These entrypoints require the matching fixed workspace and revision marker.
timeout -k 20 1500 sh harness/tools/hashtag-runtime/run.sh
# If outer timeout interrupts EXIT cleanup, remove only this task's resources:
timeout -k 10 180 sh harness/tools/hashtag-runtime/cleanup.sh
```

Common bounds: build 4 CPU/6 GiB/512 PIDs/870s + 900s outer; PG 1 CPU/512 MiB/
128 PIDs/5400s; Rust web 2 CPU/2 GiB/256 PIDs/5000s; full invocation 1500s.
Browser: 2 CPU/2 GiB; TLS 1 CPU/256 MiB; forward 1 CPU/128 MiB; helpers 256 PIDs/
1200s; controller 600s; CLI commands 35s + kill grace. Rails: 2 CPU/2 GiB/
256 PIDs/600s; Redis 1 CPU/256 MiB/64 PIDs/600s; HTTP comparator 1 CPU/256 MiB/
128 PIDs/220s + 240s outer; differential setup/requests 420s outer.

Internal networks; no published host ports, no Rust or Sidekiq workers, no
internet federation. Deferred AddHashtag/RemoveHashtag jobs may enqueue in task
Redis but never deliver. Only browser loopback TLS forwarder gets
NET_BIND_SERVICE. PG **14.23**; Rust runtime/writer have no ownership, elevated
role flags or memberships; owner used only for restore/migration/grants/setup.
Rails has its own DB owner role (not superuser/role creator/database creator),
`hashtag_rails` DB and media clone, separate from Rust's `remote_browser` DB/media.
Roles/DB owners and exact source/binary/image identities are retained.

Images: tools `7203e0222e2b`, browser `d6337b96fb60`, PG `1a6c2409ab71`,
Rails immutable child `696439e1ada71d0cf3d51d4d6a4744d6e40b57aafa64980b18f4d3b78230d0cf`,
Redis child `9702d01c1f10c3ea9f48211b4362e44f154ff02d063e6f7268eba804059f53bf`.

## Evidence, checks and remaining gates

Sanitized final exports: ignored `target/hashtag-acceptance-bd01aca/{browser,differential}`.
They include actual responses/clicks, screenshots, source/image/binary hashes,
role proof, summary and teardown, with SHA256 checksums. Executed client/helper
hashes and application source match local files. Final generator reproduces both
executed runtime scripts byte-for-byte (browser mode ran before differential
mode was added). NAS retains prior attempt directories separately. No raw service
logs, key/env/session files, binaries or response credentials exported. Credential
scan only flags the static asset filename `password-ByJdIm8f.png: OK` when using
broad keyword matching; actual credential-pattern scan is clean.

Task containers, PG volumes, networks, media, TLS keys/env/sessions and build
output removed; absence verified for both final runs. Final container state
recorded running/non-OOM before teardown. No production access, deploy or push.

Focused final offline tests: **4 adapter/controller/comparison tests + 8 existing
remote-browser regressions passed**; JS/shell syntax and diff whitespace checks
passed. `mise run harness-tests` was attempted but stops in an existing peer
harness test on macOS BSD `stat -c`; **full aggregate not passed**. No portability
fix or unrelated test expansion. TDD red revision/adapter wiring preceded green;
real failing browser/differential evidence is preserved, not recast as success.

Independent parent review **75f770** accepted the bounded harness with no blockers.
The acceptance child is archived, as are the reviewed profile-read prerequisite
(`c493`, `bd01aca`) and reviewed local API child (`8e22f52`, API/stream evidence
only). Historical pending-review notes are superseded, not evidence of extra runs.

The hashtag parent remains **OPEN** for [featured-tag limit metadata / typed-name
editor diagnosis](../../meta/issues/diagnose-featured-tag-limit-metadata-and-typed-name-editor.md).
Captured `profile-remove-reload.json` shows the maximum-tags warning with an empty
editor. Suggestion Add/Delete passed, but **typed entry is not established**. Source
has a zero fallback for missing frontend limit metadata; existing Rust limit literals
do not establish correct nesting/state. No diagnosis implementation is included.

No full parent/release/peer/check completion claim. Existing home WebSocket test
is separate, not rerun; representative browser home behavior is established here.
Peer AddHashtag/RemoveHashtag and Redis-history semantic equivalence remain deferred.
