#!/usr/bin/env python3
"""Offline control-flow checks for the standalone bootstrap fixture lane."""

from __future__ import annotations

import os
import pathlib
import subprocess
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[2]
RUNNER = ROOT / "tools" / "standalone-bootstrap-fixture"
PIN = "sha256:525844ca03edbc43a4c5fb8ca09ddef2a82bd96a9b4f826542833b7183d44c60"


def write_executable(path: pathlib.Path, body: str) -> None:
    path.write_text(body)
    path.chmod(0o755)


def run_lane(
    cargo_exit: int, collision: str = ""
) -> tuple[subprocess.CompletedProcess[str], str]:
    with tempfile.TemporaryDirectory(prefix="rustodon-bootstrap-harness-") as temporary:
        temporary_path = pathlib.Path(temporary)
        fake_bin = temporary_path / "bin"
        fake_bin.mkdir()
        log = temporary_path / "commands.log"
        write_executable(
            fake_bin / "cargo",
            f"""#!/bin/sh
printf 'cargo %s\\n' "$*" >> "{log}"
if [ "${{HARNESS_SECRET-}}" ]; then printf 'leaked-secret\\n' >> "{log}"; fi
if [ "${{RUSTODON_BOOTSTRAP_MEDIA_ROOT-}}" ]; then
  printf 'media-root=%s\\n' "$RUSTODON_BOOTSTRAP_MEDIA_ROOT" >> "{log}"
fi
case " $* " in
  *' --list '*) printf 'standalone_bootstrap_installs_and_verifies_exact_baseline: test\\n'; exit 0 ;;
esac
exit {cargo_exit}
""",
        )
        write_executable(
            fake_bin / "timeout",
            f"""#!/bin/sh
printf 'timeout %s\\n' "$*" >> "{log}"
shift 3
exec "$@"
""",
        )
        write_executable(
            fake_bin / "sleep",
            "#!/bin/sh\nexit 0\n",
        )
        write_executable(
            fake_bin / "podman",
            f"""#!/bin/sh
printf 'podman %s\\n' "$*" >> "{log}"
last=''
for argument in "$@"; do last=$argument; done
case "$1 $2" in
  'image inspect') printf 'linux/amd64|{PIN}\\n'; exit 0 ;;
  'container exists'|'volume exists'|'network exists')
    kind=$1
    [ "{collision}" = "$kind" ] && exit 0
    [ -f "{temporary_path}/$kind" ] && [ "$(cat "{temporary_path}/$kind")" = "$3" ] && exit 0
    exit 1 ;;
  'container inspect'|'volume inspect'|'network inspect')
    case "$1" in
      network) session=${{3#rustodon-standalone-bootstrap-network-}} ;;
      *) session=${{3#rustodon-standalone-bootstrap-postgres-}} ;;
    esac
    printf '%s\\n' "$session"
    exit 0 ;;
  'network create') printf '%s' "$last" > "{temporary_path}/network" ;;
  'volume create') printf '%s' "$last" > "{temporary_path}/volume" ;;
  'network rm') rm -f "{temporary_path}/network" ;;
  'volume rm') rm -f "{temporary_path}/volume" ;;
  'port rustodon-'*) printf '127.0.0.1:55432\\n' ;;
esac
if [ "$1" = run ]; then
  previous=''
  for argument in "$@"; do
    if [ "$previous" = '--name' ]; then printf '%s' "$argument" > "{temporary_path}/container"; fi
    previous=$argument
  done
elif [ "$1" = rm ]; then
  rm -f "{temporary_path}/container"
fi
exit 0
""",
        )
        env = os.environ.copy()
        env.update(
            {
                "PATH": f"{fake_bin}:{env['PATH']}",
                "HARNESS_LOG": str(log),
                "HARNESS_SECRET": "must-not-reach-cargo",
                "DATABASE_URL": "postgresql://production.invalid/never",
                "RUSTODON_BOOTSTRAP_DATABASE_URL": "postgresql://production.invalid/never",
            }
        )
        result = subprocess.run(
            [str(RUNNER)],
            cwd=ROOT,
            env=env,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        contents = log.read_text() if log.exists() else ""
        media_lines = [line for line in contents.splitlines() if line.startswith("media-root=")]
        if media_lines:
            media_root = pathlib.Path(media_lines[-1].split("=", 1)[1])
            assert not media_root.parent.exists(), f"task media root leaked: {media_root.parent}"
        return result, contents


def main() -> None:
    source = RUNNER.read_text()
    for required in [
        "--publish 127.0.0.1::5432",
        "--cpus 2",
        "--memory 2g",
        "--pids-limit 512",
        "POSTGRES_USER=$CLUSTER_USER",
        "CREATE ROLE $DATABASE_USER LOGIN NOINHERIT",
        "NOSUPERUSER",
        "NOINHERIT",
        "RUSTODON_BOOTSTRAP_DATABASE_URL",
        "RUSTODON_BOOTSTRAP_CLUSTER_ADMIN_DATABASE_URL",
        "RUSTODON_BOOTSTRAP_RUNTIME_DATABASE_URL",
        "RUSTODON_BOOTSTRAP_WRITER_DATABASE_URL",
        "RUSTODON_BOOTSTRAP_RUNTIME_ROLE",
        "RUSTODON_BOOTSTRAP_WRITER_ROLE",
        "RUSTODON_BOOTSTRAP_MEDIA_ROOT",
        "standalone_bootstrap_installs_and_verifies_exact_baseline",
        "--ignored --exact --nocapture --test-threads=1",
        "trap cleanup EXIT",
        "trap 'exit 143' TERM",
    ]:
        assert required in source, f"runner is missing contract: {required}"
    for forbidden in ["mastodon/mastodon", "bundle exec", "redis", "Sidekiq", "fixture-obtain"]:
        assert forbidden not in source, f"standalone lane gained forbidden dependency: {forbidden}"

    success, success_log = run_lane(0)
    assert success.returncode == 0, success.stderr
    assert "leaked-secret" not in success_log
    assert "cargo test --locked --features test-support --test standalone_bootstrap" in success_log
    assert success_log.index(" --list ") < success_log.index("podman network create")
    assert "podman rm -f --volumes rustodon-standalone-bootstrap-postgres-" in success_log
    assert "podman volume rm -f rustodon-standalone-bootstrap-postgres-" in success_log
    assert "podman network rm rustodon-standalone-bootstrap-network-" in success_log
    timeout_calls = sum(line.startswith("timeout ") for line in success_log.splitlines())
    podman_calls = sum(line.startswith("podman ") for line in success_log.splitlines())
    assert timeout_calls >= podman_calls

    failure, failure_log = run_lane(23)
    assert failure.returncode == 23, (failure.returncode, failure.stderr)
    assert "podman rm -f --volumes rustodon-standalone-bootstrap-postgres-" in failure_log
    assert "podman volume rm -f rustodon-standalone-bootstrap-postgres-" in failure_log
    assert "podman network rm rustodon-standalone-bootstrap-network-" in failure_log

    for collision in ["container", "volume", "network"]:
        rejected, collision_log = run_lane(0, collision)
        assert rejected.returncode != 0, (collision, rejected.stderr)
        assert f"podman {collision} rm" not in collision_log
        if collision == "container":
            assert "podman rm -f --volumes" not in collision_log
    print("standalone bootstrap fixture harness checks passed")


if __name__ == "__main__":
    main()
