#!/usr/bin/env python3
"""Offline real-shell wiring regression; no fixture workload.

--fixture-script PATH permits red-before-change evidence against a saved script.
Cargo, Podman, database/server setup, and cleanup are stubs; no real workloads.
Discovered automatically by tools/check-harnesses.
"""
import argparse
import os
from pathlib import Path
import shlex
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
parser = argparse.ArgumentParser(add_help=False)
parser.add_argument("--fixture-script", type=Path,
                    default=ROOT / "tools/mastodon-fixture")
options, unittest_args = parser.parse_known_args()
SCRIPT = options.fixture_script.resolve()

RUNNER = r'''
set -eu
MASTODON_FIXTURE_LIBRARY=1
. "$FIXTURE_SCRIPT"
ROOT=$SANDBOX
FIXTURE_DIR="$ROOT/fixtures"
# Real differential_test control flow, with every external service replaced.
verify_static() { :; }
verify_differential_images() { :; }
start_database() {
  printf 'provision|database\n' >> "$CALLS"
  PG_CONTAINER=offline-postgres
}
prepare_differential_database() { :; }
prepare_differential_writer_database() { :; }
start_differential_redis() { :; }
run_worker_cli() { printf 'cli|%s\n' "${CARGO_PROFILE_TEST_OPT_LEVEL-unset}" >> "$CALLS"; }
start_differential_web() {
  WEB_CONTAINER=offline-web
  printf 'media|%s\n' "$3" >> "$CALLS"
}
reset_differential_feeds() { :; }
finish_differential() { printf 'finish\n' >> "$CALLS"; }
run_podman() {
  case "$1" in
    exec) cat >/dev/null ;;
    port)
      case "$3" in
        5432/tcp) printf '127.0.0.1:15432\n' ;;
        3000/tcp) printf '127.0.0.1:13000\n' ;;
        *) exit 80 ;;
      esac ;;
    *) echo 'unexpected offline Podman operation' >&2; exit 80 ;;
  esac
}
cargo() {
  printf 'cargo|%s|%s\n' "${CARGO_PROFILE_TEST_OPT_LEVEL-unset}" "$*" >> "$CALLS"
  case " $* " in
    *' --list '*)
      case "$FAIL_MODE" in
        empty) return 0 ;;
        list-error) return 7 ;;
      esac
      printf 'mixed_profile_media: test\n1 test, 0 benchmarks\n'
      ;;
    *' --test differential mixed_profile_media '*)
      [ "$FAIL_MODE" != run-error ] || return 9 ;;
  esac
}
differential_test "$SELECTOR"
printf 'after|%s\n' "${CARGO_PROFILE_TEST_OPT_LEVEL-unset}" >> "$CALLS"
'''


class MixedProfileHarness(unittest.TestCase):
    def run_harness(self, selector, fail_mode="present", inherited=None):
        with tempfile.TemporaryDirectory(prefix="mixed-profile-wiring-") as tmp:
            root = Path(tmp)
            (root / "target").mkdir()
            (root / "tools").mkdir()
            verifier = root / "tools/verify-worker-media"
            verifier.write_text(
                '#!/bin/sh\nset -eu\nprintf "%s\\n" verify-worker-media >> "$CALLS"\n'
            )
            verifier.chmod(0o755)
            (root / "fixtures/media").mkdir(parents=True)
            for filename in ("database.sql", "verify.sql"):
                (root / "fixtures" / filename).write_text("")
            calls = root / "calls"
            env = {
                "PATH": os.environ["PATH"],
                "HOME": tmp,
                "FIXTURE_SCRIPT": str(SCRIPT),
                "SANDBOX": tmp,
                "CALLS": str(calls),
                "SELECTOR": selector,
                "FAIL_MODE": fail_mode,
            }
            if inherited is not None:
                env["CARGO_PROFILE_TEST_OPT_LEVEL"] = inherited
            result = subprocess.run(["/bin/sh", "-c", RUNNER], env=env,
                                    capture_output=True, text=True, timeout=10)
            lines = calls.read_text().splitlines() if calls.exists() else []
            self.assertEqual(lines[:1], ["verify-worker-media"], result.stderr)
            self.assertEqual(lines.count("verify-worker-media"), 1, lines)
            cargo = [(parts[1], shlex.split(parts[2]))
                     for line in lines if line.startswith("cargo|")
                     for parts in [line.split("|", 2)]]
            return result, lines, cargo

    def assert_mixed_runs_once_optimized(self, cargo):
        mixed = [(profile, args) for profile, args in cargo
                 if "mixed_profile_media" in args
                 and "--skip" not in args]
        self.assertEqual(len(mixed), 2, mixed)  # discovery, then execution
        self.assertIn("--list", mixed[0][1])
        self.assertNotIn("--list", mixed[1][1])
        for profile, args in mixed:
            self.assertEqual(profile, "3")
            for flag in ("--locked", "--ignored", "--exact"):
                self.assertIn(flag, args)
            self.assertEqual(args[args.index("--features") + 1], "test-support")
            self.assertEqual(args[args.index("--test") + 1], "differential")
        self.assertIn("--test-threads=1", mixed[1][1])

    def test_selected_mixed_optimizes_discovery_and_execution_only(self):
        for inherited in (None, "1"):
            with self.subTest(inherited=inherited):
                result, lines, cargo = self.run_harness("mixed_profile_media", inherited=inherited)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assert_mixed_runs_once_optimized(cargo)
                original = inherited or "unset"
                self.assertIn(f"cli|{original}", lines)
                self.assertIn(f"after|{original}", lines)
                self.assertIn("media|rw", lines)
                for profile, args in cargo:
                    if args[0] == "build":
                        self.assertEqual(profile, original)

    def test_unrelated_selector_retains_callers_profile(self):
        result, lines, cargo = self.run_harness("media_writes", inherited="1")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(cargo)
        self.assertTrue(all(profile == "1" for profile, _ in cargo), cargo)
        self.assertIn("after|1", lines)

    def test_all_cases_skip_unoptimized_mixed_then_run_it_once_optimized(self):
        result, lines, cargo = self.run_harness("")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assert_mixed_runs_once_optimized(cargo)
        self.assertIn("media|rw", lines)
        self.assertIn("after|unset", lines)
        batch = [(profile, args) for profile, args in cargo
                 if "--skip" in args and "--test" in args]
        self.assertEqual(len(batch), 1, batch)
        self.assertEqual(batch[0][0], "unset")
        self.assertIn(["--skip", "mixed_profile_media"],
                      [batch[0][1][i:i+2] for i in range(len(batch[0][1]) - 1)])
        executed = [(profile, args) for profile, args in cargo
                    if args[0] == "test" and "--list" not in args]
        self.assertEqual(executed[-1][0], "3", "mixed must run after existing comparisons")
        self.assertIn("mixed_profile_media", executed[-1][1])
        for profile, args in cargo:
            if "mixed_profile_media" not in args or "--skip" in args:
                self.assertEqual(profile, "unset", args)

    def test_missing_or_failed_discovery_fails_before_setup(self):
        for selector in ("mixed_profile_media", ""):
            for mode in ("empty", "list-error"):
                with self.subTest(selector=selector, mode=mode):
                    result, lines, cargo = self.run_harness(selector, mode)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertEqual(len(cargo), 1, cargo)
                    self.assertIn("--list", cargo[0][1])
                    self.assertFalse(any(line.startswith("media|") for line in lines))
                    self.assertNotIn("finish", lines)

    def test_mixed_failure_propagates_in_selected_and_all_case_modes(self):
        for selector in ("mixed_profile_media", ""):
            with self.subTest(selector=selector):
                result, lines, _ = self.run_harness(selector, "run-error")
                self.assertEqual(result.returncode, 9, result.stderr)
                self.assertNotIn("finish", lines)


if __name__ == "__main__":
    unittest.main(argv=[sys.argv[0], *unittest_args])
