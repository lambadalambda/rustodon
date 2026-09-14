#!/bin/bash
# Sourced by the disposable peer runner and its offline control-flow tests.
FIXTURE_SUMS_SHA256=cec12d2c7d66c75d82b3f5191b8363cc8bdcb1882dc8f27df26e5e9609526c81
peer_contract() {
  local marker_mode marker_owner
  [[ ${RUSTODON_PEER_SMOKE:-} == 1 ]] ||
    die 'set RUSTODON_PEER_SMOKE=1 to authorize the manual peer smoke'
  [[ $1 == "$2" ]] || die 'peer workdir and physical repository root differ'
  [[ -f $3 && ! -L $3 ]] ||
    die 'peer smoke requires a regular target/.rustodon-peer-workspace marker'
  marker_mode=$(stat -c '%a' "$3") || die 'cannot inspect peer workspace marker mode'
  marker_owner=$(stat -c '%u' "$3") || die 'cannot inspect peer workspace marker owner'
  [[ $marker_mode == 600 && $marker_owner == "$(id -u)" ]] ||
    die 'peer workspace marker must be mode 600 and owned by the invoking user'
  [[ $(cat "$3") == "rustodon-peer-workspace-v1:$2" ]] ||
    die 'peer workspace marker does not authorize this physical repository root'
}

peer_prerequisites() {
  local fixture_sums image
  fixture_sums=$(sha256sum "$FIXTURE_DIR/SHA256SUMS") ||
    die 'cannot checksum the committed peer fixture inventory'
  [[ ${fixture_sums%% *} == "$FIXTURE_SUMS_SHA256" ]] ||
    die 'peer fixture inventory differs from the committed contract'
  verify_static "$FIXTURE_DIR" || die 'invalid committed peer fixture'
  for image in "$MASTODON_IMAGE" "$POSTGRES_IMAGE" "$REDIS_IMAGE"; do
    [[ $image =~ @sha256:[0-9a-f]{64}$ ]] || die "peer image is not immutable: $image"
    run_podman image exists "$image" || die "missing pinned image: $image"
    [[ $(run_podman image inspect "$image" --format '{{.Os}}/{{.Architecture}}|{{.Digest}}') == "$PLATFORM|${image##*@}" ]] ||
      die "cached image platform/digest mismatch: $image"
  done
  printf 'verified generic cache-only peer prerequisites\n'
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
  env -i PATH="$PATH" HOME="${HOME:-/tmp}" XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-}" \
    podman --remote=false "$@"
}

peer_podman() {
  case "$1" in
    run)
      shift
      peer_engine run --pull=never --memory=1200m --cpus=1 --pids-limit=256 "$@"
      ;;
    pull|build) die 'peer lane forbids image retrieval/builds' ;;
    *) peer_engine "$@" ;;
  esac
}

# Tooling containers can reuse process IDs on each invocation; retain numeric
# marker validation while avoiding collisions with evidence from previous runs.
peer_run_id() { printf '%s%s\n' "$$" "$(date +%s%N)"; }

peer_resource_inventory() {
  local run=$1 pid=$2
  printf 'container %s\n' \
    "rustodon-fixture-v4-6-5-postgres-$pid" \
    "rustodon-differential-v4-6-5-redis-$pid" \
    "$run-probe" "$run-seed-mastodon" "$run-seed-rustodon" \
    "$run-web" "$run-sidekiq"
  printf 'network %s\n' "rustodon-fixture-v4-6-5-network-$pid"
  printf 'volume %s\n' "rustodon-fixture-v4-6-5-postgres-$pid"
}
