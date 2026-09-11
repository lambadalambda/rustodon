# Pinned Pleroma peer build prerequisite

Owned prerequisite of `meta/issues/add-isolated-federation-peer-tests.md`.
This page is the scoped build log/provenance; shared issue/index files and the
shared peer harness are intentionally untouched.

## Current result: blocked, no image

On Secunda, neither exact release base was cached. One anonymous request was
made for the exact Elixir tag, using an explicit empty Podman auth file and
`--retry=0`. It failed with exit 125:

```text
reading manifest 1.17.3-erlang-26.2.5.6-alpine-3.17.9 in docker.io/hexpm/elixir:
toomanyrequests: You have reached your unauthenticated pull rate limit.
```

No retry, alternate registry, credentials, or Alpine request followed. The
helper's subsequent `--build` guard only inspected the local image store and
failed on the missing image; it did not contact a registry.

| Artifact | Verified result |
| --- | --- |
| Pleroma source | Exact v2.10.2 Git archive prepared and checksum-checked |
| Elixir base digest | **Unresolved: registry quota** |
| Alpine base digest | **Unresolved: not requested after quota blocker** |
| Hex/Rebar/native package versions | **Not installed or measured** |
| Resulting Pleroma image ID | **None: build never started** |
| Fresh DB migrations/account/API token | **Not attempted: no image** |
| Federation/TLS interoperability | **Not implemented or claimed** |

The build path after base resolution is supplied for continuation but remains
**unexecuted**. Guard tests and archive preparation are not image-build coverage.
Do not hand the sibling harness a floating Pleroma tag as a substitute.

## Exact source provenance

Source repository (read only):
`/Users/lainsoykaf/repos/pleroma-org/pleroma`.
Locally verified `git rev-parse 'v2.10.2^{commit}'`:

```text
cd8816eccec355b5ff1533dc75fc86e9927baf2b
```

Exported with `git archive --format=tar` of that **commit**, not the working
checkout/branch. Only that Git tree crossed to Secunda: no `.git`, dependency
cache, `_build`, working-tree configuration secrets, or instance files. Git tar
commit metadata on Secunda independently returned the same revision.

SHA-256, measured on Secunda:

```text
bb5c9cd6e6c6be270b119f12e5879710a496f12d1015d5448bb5c925a402046e  source.tar
9e516ac617c5c95711b489560e139e31dcec5e43fca84556f83d8497e1347eba  Dockerfile
3e44cbaa4ed88ee13e1992345cba44d8bc9b9348fb43dc41036e9a8fe93ab456  mix.lock
9c76c3d391c1ba78f84eccdd31db28c3d826b4bdceaea95feaa78a793f62ae4a  mix.exs
```

The original release Dockerfile specifies:

- `docker.io/hexpm/elixir:1.17.3-erlang-26.2.5.6-alpine-3.17.9`
- `docker.io/library/alpine:3.17.9`
- Builder APKs: git, gcc, g++, musl-dev, make, cmake, file-dev, vips-dev.
- Runtime APKs: exiftool, ffmpeg, vips, libmagic, ncurses, postgresql-client.
- `MIX_ENV=prod`, `VIX_COMPILATION_MODE=PLATFORM_PROVIDED_LIBVIPS`.
- `mix local.hex --force`, `mix local.rebar --force`, release-locked production
  dependency resolution, and `mix release --path release`.

The checkout's newer Dockerfile must not be used. Without `.git`, the release
`mix.exs` version derivation falls back to `2.10.2`, without branch metadata.
The immutable archive checksum deliberately fails closed even if a different
Git/archive implementation emits different tar bytes: investigate provenance
rather than bypassing the check.

## Continuation commands

All execution below is on `lain@secunda.local`, as unprivileged `lain`, under
`/home/lain/rustodon-parity/pleroma-peer`. Python 3.12+ and rootless Podman are
required. The main task source is already mirrored there; synchronize only
these explicitly owned/tracked helper files when updating it.

The archive is already at `pleroma-artifacts/source.tar`. If it must be
re-exported, this is the only local source-export command (do not rsync the
Pleroma checkout):

```sh
git -C /Users/lainsoykaf/repos/pleroma-org/pleroma archive --format=tar \
  cd8816eccec355b5ff1533dc75fc86e9927baf2b |
  ssh lain@secunda.local \
    'cat > /home/lain/rustodon-parity/pleroma-peer/pleroma-artifacts/source.tar'
```

Archive preparation and offline guards:

```sh
cd /home/lain/rustodon-parity/pleroma-peer
python3 tools/federation-pleroma-build-test.py
tools/federation-pleroma-build pleroma-artifacts/source.tar \
  pleroma-artifacts/prepare-next
```

Output directories must be new and beneath this task workspace. Nothing is
removed or overwritten by the helper. Preparation needs no Podman or network.

**Only after the quota blocker is cleared and another attempt is authorized:**
inspect the cache first, then obtain each missing exact base once, with
`podman pull --retry=0 --authfile pleroma-artifacts/anonymous-auth.json EXACT_TAG`.
That task-owned auth file contains only `{"auths":{}}`. Stop immediately on
quota failure; do not loop or use stored registry credentials. Keep those
pull logs with the build evidence. Do not substitute a nearby version.

Then:

```sh
tools/federation-pleroma-build pleroma-artifacts/source.tar \
  pleroma-artifacts/build-next --build
```

The helper inspects the **cached exact release tags** and requires one matching
repository SHA-256 digest and linux/amd64 metadata for each. It writes the
complete base inspection records and digest pins **before building**. Generated
`Containerfile.pinned` replaces both `FROM` lines with immutable references.
Podman runs with `--pull=never --retry=0 --no-cache`; missing bases fail closed.
The tool does not itself resolve remote tags or retry pulls.

The release Dockerfile's install/build commands remain unchanged. Added
provenance steps verify that dependency resolution/build did **not** alter
`mix.lock`, copy that exact lockfile into the release, and capture Elixir/OTP,
Hex, Rebar, and complete builder/runtime APK installed-version inventories.
Normal dependency downloads are allowed; no second Pleroma source checkout is
used. The generated build recipe is separate from the pristine extracted tree.

Successful completion prints the resulting **local image ID**, not a floating
tag. The sibling harness should use that ID on this host. The output directory
contains:

- `provenance.json`: source checksums, base digests, generated-recipe checksum,
  build command/date, Podman version, completion status, and image ID.
- `elixir-base.json`, `alpine-base.json`, `Containerfile.pinned`, `build.log`.
- `image.id`, `image-inspect.json`, `toolchain-packages.txt` (including lockfile).

The collection smoke runs the image with `--network=none`, bypassing the
application entrypoint, and checks the embedded lock checksum. It is **not** an
application startup or database smoke. The release's Hex/Rebar installers and
Alpine package repositories are not version-pinned by upstream; recording their
actual versions is provenance, not a claim of byte-reproducible future builds.
Exact source/lock plus resolved bases alone do not make a reproducible image.

## Executed checks and artifacts

TDD for the script guards: remote red failed because the helper did not exist;
remote green passed all three tests (wrong archive, immutable cached-base
validation, and generated Dockerfile invariants). Remote `py_compile` passed.
Real archive preparation passed. An additional remote smoke transformed the
**actual archived release Dockerfile** with placeholder digests and asserted
the two immutable FROM lines, provenance stage placement, original release
commands, and `/release` stage-copy destination; it did not build an image.
Real `--build` failed closed on the missing cached Elixir base, recording a
failed status without an image ID. The actual container-build/provenance path
needs an executed smoke after the quota clears; classic red/green cannot
substitute for that external build evidence.

Independent read-only review found no blocking defect. Deferred nonblocking
hardening: mocked orchestration-order/flag regression tests, and recording
unexpected non-subprocess exceptions in the failure manifest. Neither is
claimed as covered by the current three guard tests.

Retained only under the task-owned Secunda directory:

```text
pleroma-artifacts/source.tar
pleroma-artifacts/source/                 # initial exact export inspection
pleroma-artifacts/base-pull.log           # single failed registry attempt
pleroma-artifacts/anonymous-auth.json     # empty auths, not credentials
pleroma-artifacts/prepared/               # successful helper preparation
pleroma-artifacts/build-attempt/          # local-cache guard failure manifest
pleroma-artifacts/build-guard.log
```

No peer containers, databases, networks, or volumes were created. No broad
pruning, live origins, tunnels, environment files, backups, or edits beneath
`/home/lain/repos/rustodon` were involved.

## Bootstrap handoff boundary

Minimal later topology is the release plus its **own PostgreSQL**; Oban runs
inside the application. No Redis or frontend installation is required for the
API bootstrap. The release entrypoint waits for PostgreSQL, runs
`pleroma_ctl migrate`, then starts the release. A later disposable proof must
actually verify fresh migrations and create a **nonadmin** account/API token;
no helper for this is supplied without a runnable image to validate it.

Shared scenario running, TLS routing, Rustodon transport, and other-peer
bootstrap are outside this prerequisite. Future test-CA configuration must
preserve `verify_peer`, hostname verification, and per-host SNI. Hackney's
nonempty `ssl_options` replace defaults: supplying a CA alone must not silently
drop those checks. This change supplies no TLS overrides or federation claim.
