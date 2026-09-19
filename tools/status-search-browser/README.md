# Focused status-search browser acceptance

Owner: [bounded subissue](../../meta/issues/accept-status-search-browser.md).
Application: **c3497f5cdf71678c70542689e079ac589de253a1**. This is a focused
acceptance controller, not the full browser integration lane or a deployment tool.

`prepare.py` materializes a fixed adaptation of the existing `remote-browser`
launcher/setup/cleanup in the **task workspace only**. Existing tracked media
acceptance files are unchanged. It removes media acceptance and workers, selects
this source/controller, and adds migration/privacy checks. TLS, browser image,
login, fixture roles and cleanup are reused. No generic framework, response
injection, Redux manipulation, frontend patch, or application change.

## Fresh launch (Mac → isolated NAS)

Load `agent-browser` and `nas-podman` skills first. Keep SSH connections sequential.
Stop on any failed command; if the SSH agent is locked, ask to unlock it.

```bash
set -euo pipefail
REV=c3497f5cdf71678c70542689e079ac589de253a1
W=/srv/workspaces/rustodon-status-search-browser-c3497f5-alice
# Reuses the clean exact reference, never modifies/fetches another revision.
tools/mastodon-fixture verify-source target/mastodon-v4.6.5
PYTHONDONTWRITEBYTECODE=1 STATUS_SEARCH_PINNED_SOURCE="$PWD/target/mastodon-v4.6.5" \
  python3 -W error tools/tests/status-search-browser-test.py

git archive --format=tar "$REV" | ssh root@podman-worker.local \
  "test ! -e '$W' && mkdir -p '$W/source' '$W/harness' '$W/evidence' && tar -xf - -C '$W/source' && printf '%s\\n' '$REV' > '$W/evidence/application-revision.txt'"
COPYFILE_DISABLE=1 tar -cf - $(git ls-files tools/remote-browser) \
  tools/status-search-browser/prepare.py tools/status-search-browser/source.py \
  tools/status-search-browser/ui.py tools/status-search-browser/observe.js \
  tools/tests/status-search-browser-test.py | ssh root@podman-worker.local \
  "tar -xf - -C '$W/harness' && cd '$W' && python3 harness/tools/status-search-browser/prepare.py harness harness/tools/status-search-runtime"
tools/mastodon-fixture verify-source target/mastodon-v4.6.5 | \
  ssh root@podman-worker.local "cat > '$W/evidence/reference-source.txt'"
ssh root@podman-worker.local \
  "cd '$W' && timeout -k 20 1500 sh harness/tools/status-search-runtime/run.sh > evidence/run.log 2>&1"
```

Check **each pipeline member's status** (or use a shell with `set -o pipefail`).
The fixed destination refuses an existing workspace; do not overwrite evidence.
After a failed controller, retain the attempt separately before a fresh run. The
launcher refuses existing task containers/network/fixture state. If the outer
wall timeout interrupts its EXIT trap, invoke only its generated cleanup:

```sh
ssh root@podman-worker.local \
  "cd '$W' && timeout -k 10 180 sh harness/tools/status-search-runtime/cleanup.sh"
```

No broad pruning or production access. Do not sync instance environments, `.git`,
credentials, backups, existing build outputs or unrelated untracked files.

## Executed evidence

Final fresh uninterrupted run passed; hashes of executed harness and representative
application files match the local checkout. Evidence is ignored/uncommitted under
`target/status-search-browser-evidence/evidence/`, with NAS original under `$W`.
`SHA256SUMS` verifies the exported final artifacts. Earlier evidence is separate:
`startup-attempt-evidence` failed because the harness wrongly expected Alice's
signing key; `first-pass-evidence` passed before adding whole-source request
counting and the stronger before/after privacy snapshot. Neither substitutes for
the final run.

- Alice logs in through the actual frontend. Native input + Enter for known local
  canonical URL, uncached canonical remote Note, cached repeat, known hidden URL
  and unrelated uncached private URL. Posts tab is clicked normally.
- Passive XHR observer retains only search parameters, result identities, HTTP
  code and reload identity; no headers, tokens, arbitrary response bodies or HAR.
  Every search shows real `resolve=true`, `limit=11`, absent offset, both omitted
  type and `type=statuses`. No synthetic API responses.
- Known and uncached public results are visible. Timestamp click navigates to the
  exact returned status ID in its local frontend permalink; reload independently
  obtains the same status via the actual frontend request and renders its text.
- Task TLS source returns canonical matching ID/actor Notes; existing fixture Bob
  is the valid author. RSA signatures are cryptographically verified against the
  **instance actor** public key, not the searching user's. One public object GET;
  cached repeat and known-hidden denial each have zero additional source requests.
  Every source request except the single readiness path is counted, including
  unexpected paths (redacted). This is controlled TLS/API evidence, not a real-peer
  or unknown-actor-discovery gate.
- Both denied URLs render `No results.` and no status DOM. Private Note addresses
  Carol only, not Alice. Two private GETs (All, Posts) are expected because denied
  objects are not persisted. SQL confirms public count 1, private count 0, new
  task-status mentions 0, total mentions unchanged at 8, known-hidden row hash
  unchanged. Fresh baseline confirms neither task object was persisted beforehand.
- PG **14.23**, operational migration **5**; restricted runtime/writer are neither
  superuser nor role/database creators/BYPASSRLS and have zero role memberships.
  Owner credentials are confined to restore/grants/observations/migration.
- CA-verified readiness, pinned frontend pack checksums, image identities, browser
  version, screenshots, query observations, SQL and source counts are retained.
  Credential-pattern scan passed before export; no session or private-key files
  exported. Browser closed, task containers/PG volume/network/media/certs/keys/env
  and fresh build output removed; no task containers/network remained.

## Bounds and exclusions

Serial build: 4 CPU / 6 GiB / 512 PIDs / 870s container + 900s outer timeout.
PG: 1 CPU / 512 MiB / 128 PIDs / 5400s; web: 2 CPU / 1 GiB / 256 PIDs / 5000s.
Browser: 2 CPU / 2 GiB; source/TLS: 1 CPU / 256 MiB; forward: 1 CPU / 128 MiB;
runtime helpers: 256 PIDs / 1200s. Total launcher bound 1500s; controller 600s;
browser commands 35s + kill grace. Internal network, no published host ports.
Only canonical loopback TLS forwarder gains NET_BIND_SERVICE. Debug/test-support
origin mapping uses task CA without weakening production SSRF/TLS/signature rules.

Offline helper/source tests: **5 passed**, including an opt-in clean pinned
Mastodon query/source contract and RSA positive/tampered/wrong-actor cases.
Existing media harness regressions: **8 passed**. Initial missing-helper RED and
wrong-signer RED preceded fixes; browser acceptance tests existing app behavior.

Singleton canonical URL results cannot expose a load-more control. Initial
pagination is observed; expansion offset/limit and Rails typed-offset semantics
are checked against exact clean source, **not browser-executed nonzero pagination**.
No full-text index, HTML alternate discovery, actor-URL resolution, hashtags,
unknown-author discovery or general client/peer parity claim. Prior HTTP security
matrix is separate and was not rerun. Full `pinned-source-contracts` Rust lane,
full browser/DB/worker/peer/differential lanes, full check/Clippy were not run here.

Independent parent review is **pending**: nested delegation was denied by this
session's depth limit. Leave all changes uncommitted; no push or deployment.
