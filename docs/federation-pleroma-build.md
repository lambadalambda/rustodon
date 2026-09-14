# Pinned Pleroma peer build prerequisite

Pleroma interoperability is not yet implemented or claimed. This document
records the exact source/build contract and the current blocker without treating
archive validation as a successful image or peer test.

## Current status

The exact Pleroma v2.10.2 source archive and lockfile are verified. The required
Elixir and Alpine base-image digests were not available in the inspected local
cache, and an anonymous request for the exact Elixir tag hit the registry rate
limit. No retry, alternate registry, credentials, nearby version, or floating
image fallback was used. No Pleroma image, database, account, API token, or
federation result exists.

| Artifact | Status |
| --- | --- |
| Pleroma source | Exact v2.10.2 archive checksum verified |
| Elixir base digest | Unresolved |
| Alpine base digest | Unresolved |
| Hex/Rebar/package versions | Not measured |
| Pleroma image | Not built |
| Database/bootstrap | Not attempted |
| Federation/TLS interoperability | Not implemented or claimed |

## Source provenance

| Artifact | SHA-256 |
| --- | --- |
| Git revision | `cd8816eccec355b5ff1533dc75fc86e9927baf2b` |
| `source.tar` | `bb5c9cd6e6c6be270b119f12e5879710a496f12d1015d5448bb5c925a402046e` |
| `Dockerfile` | `9e516ac617c5c95711b489560e139e31dcec5e43fca84556f83d8497e1347eba` |
| `mix.lock` | `3e44cbaa4ed88ee13e1992345cba44d8bc9b9348fb43dc41036e9a8fe93ab456` |
| `mix.exs` | `9c76c3d391c1ba78f84eccdd31db28c3d826b4bdceaea95feaa78a793f62ae4a` |

The canonical upstream is
[`https://git.pleroma.social/pleroma/pleroma.git`](https://git.pleroma.social/pleroma/pleroma.git).
Produce the required archive from the recorded commit, not from a working tree:

```sh
set -eu
revision=cd8816eccec355b5ff1533dc75fc86e9927baf2b
checkout="$(mktemp -d)"
trap 'rm -rf -- "$checkout"' EXIT
git clone --no-checkout https://git.pleroma.social/pleroma/pleroma.git "$checkout"
git -C "$checkout" checkout --detach "$revision"
test "$(git -C "$checkout" rev-parse HEAD)" = "$revision"
mkdir -p target/pleroma-peer
git -C "$checkout" archive --format=tar "$revision" > target/pleroma-peer/source.tar
printf '%s  %s\n' \
  bb5c9cd6e6c6be270b119f12e5879710a496f12d1015d5448bb5c925a402046e \
  target/pleroma-peer/source.tar | sha256sum -c -
```

The checksum command must report `target/pleroma-peer/source.tar: OK` before the
helper is used. The archive does not contain `.git`, dependency caches, `_build`,
instance configuration, or secrets. The helper rejects byte differences rather
than silently accepting another archive implementation or source revision.

The release Dockerfile specifies:

- `docker.io/hexpm/elixir:1.17.3-erlang-26.2.5.6-alpine-3.17.9`;
- `docker.io/library/alpine:3.17.9`;
- builder packages for Git/C/C++/CMake/file/libvips;
- runtime packages for ExifTool, FFmpeg, libvips, libmagic, ncurses, and
  PostgreSQL client tools;
- production Mix environment and platform-provided libvips;
- release-locked dependency resolution and `mix release`.

A newer branch Dockerfile is not an acceptable substitute.

## Preparing the exact archive

The helper is intentionally confined to the ignored
`target/pleroma-peer/` artifact root and refuses root execution or an existing,
out-of-root, or root-level output directory.

```console
python3 tools/federation-pleroma-build-test.py

tools/federation-pleroma-build \
  target/pleroma-peer/source.tar \
  target/pleroma-peer/prepared
```

Preparation requires no container engine or network. It verifies the archive and
lockfile, extracts into a new directory, and records `provenance.json` with the
source revision/checksums, release Dockerfile checksum, and a clear “no image
built” status.

## Building after prerequisites exist

Only after both exact release tags are already cached:

```console
tools/federation-pleroma-build \
  target/pleroma-peer/source.tar \
  target/pleroma-peer/build \
  --build
```

The helper requires unprivileged rootless Podman, forces local-engine mode, and
passes only the path, home, and rootless runtime directory to Podman. It inspects
each cached tag and requires exactly one matching repository digest with
`linux/amd64` metadata. It then writes the complete inspection records and
generates a separate `Containerfile.pinned` whose `FROM` lines use immutable
digests.

The build uses `--pull=never --retry=0 --no-cache --rm --force-rm`, two build
jobs, a 100,000 µs CPU period with a 200,000 µs quota (an actual two-CPU
ceiling), explicit memory/process limits, and a one-hour wall deadline. Missing
images or ambiguous digests fail before build; the helper never resolves tags or
contacts a registry. The release Dockerfile's install/build commands remain
intact. Added provenance steps verify that dependency resolution did not alter
`mix.lock`, copy the lock into the release, and capture Elixir/OTP, Hex, Rebar,
and complete builder/runtime package inventories.

Every Podman child starts in its own process group. A timeout or interruption
sends that group `SIGTERM`, waits five seconds, then sends `SIGKILL` if needed.
Each invocation gets a random task ID: generated build stages/intermediate
images and the final image carry its exact label, the final image has an exact
task name, and the evidence container has an exact task name and the same label.
On any failed build workflow, bounded best-effort reconciliation queries only
that exact label, validates returned IDs before removing them, and also addresses
the exact container and image names. This includes external Buildah containers
that Podman can leave if a build is killed. It never uses a broad container or
image prune.

A successful build records:

- source, lockfile, and generated-recipe checksums;
- exact base-image digests and inspection records;
- sanitized build command/date, Podman version, and task-specific resource scope;
- local image ID and image inspection;
- embedded lockfile and package/toolchain evidence.

A failure keeps full build output in `build.log` and adds a structured `failure`
object to `provenance.json`: the phase, sanitized argument-vector command,
return code (when one exists), timeout duration/flag, interruption flag, and at
most 16 KiB of the tail of combined stdout/stderr. Reconciliation commands and
results are recorded separately; cleanup failure does not replace the original
failure evidence.

The evidence collection is separately resource- and time-bounded and runs the
resulting image with `--network=none`. It does not constitute application
startup, database migration, or federation proof. Upstream package installers
and repositories are not immutable, so recorded versions are provenance rather
than a claim of future byte-for-byte image reproducibility.

## Required follow-up

A disposable bootstrap must still:

1. build the image from the exact archive and immutable bases;
2. start the release with its own PostgreSQL database;
3. run fresh migrations;
4. create a non-administrator account and API token;
5. integrate the peer topology without weakening TLS verification;
6. execute and record real interoperability scenarios.

Pleroma uses Oban inside the application and does not require Redis or a separate
frontend installation for this API bootstrap. Future test-CA configuration must
retain peer verification, hostname verification, and per-host SNI; supplying a
CA alone must not replace those checks.

See [Testing](testing.md#pleroma-status) for the evidence boundary.
