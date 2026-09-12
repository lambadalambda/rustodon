#!/bin/bash
# Sourced by the disposable peer runner and its offline control-flow tests.
peer_profile() {
  [[ $2 == "$3" ]] || die 'peer workspace and physical script root differ'
  case "$1:$2" in
    secunda:/home/lain/rustodon-parity/peer-tests) PEER_PROFILE=secunda-source ;;
    podman-worker:/srv/workspaces/rustodon-peer-tests/source) PEER_PROFILE=nas-cached-image ;;
    *) die 'peer smoke requires an exact authorized host/workspace pair' ;;
  esac
}

peer_prerequisites() {
  case "$PEER_PROFILE" in
    secunda-source)
      SOURCE_DIR=/home/lain/repos/rustodon/target/mastodon-v4.6.5
      [[ $(git -C "$SOURCE_DIR" rev-parse HEAD) == "$REVISION" ]] || die 'pinned source mismatch'
      ;;
    nas-cached-image)
      [[ $(run_podman info --format '{{.Host.Hostname}}|{{.Host.Security.Rootless}}') == 'podman-worker|false' ]] ||
        die 'NAS peer lane requires the podman-worker rootful engine'
      ;;
    *) die 'unknown peer profile' ;;
  esac
  verify_static "$FIXTURE_DIR" || die 'invalid committed peer fixture'
  for image in "$MASTODON_IMAGE" "$POSTGRES_IMAGE" "$REDIS_IMAGE"; do
    run_podman image exists "$image" || die "missing pinned image: $image"
    [[ $(run_podman image inspect "$image" --format '{{.Os}}/{{.Architecture}}|{{.Digest}}') == "$PLATFORM|${image##*@}" ]] ||
      die "cached image platform/digest mismatch: $image"
  done
  printf 'peer profile=%s (NAS uses cached images, not a source-contract check)\n' "$PEER_PROFILE"
}

peer_require_absent() {
  local status
  if run_podman "$1" exists "$2"; then
    die "refusing pre-existing peer resource: $1 $2"
  else
    status=$?
    [[ $status == 1 ]] || die "cannot establish peer resource absence: $1 $2 (exit $status)"
  fi
}

peer_engine() {
  env -i PATH="$PATH" HOME="${HOME:-/tmp}" XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-}" podman "$@"
}

peer_podman() {
  case "$1" in
    run) shift; peer_engine run --pull=never "$@" ;;
    pull|build) die 'peer lane forbids image retrieval/builds' ;;
    *) peer_engine "$@" ;;
  esac
}

# Tooling containers can reuse PID 7 on each invocation; retain numeric marker
# validation while avoiding collisions with evidence from previous runs.
peer_run_id() { printf '%s%s\n' "$$" "$(date +%s%N)"; }
