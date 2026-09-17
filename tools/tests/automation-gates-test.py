#!/usr/bin/env python3
"""Offline automation contracts; no real fixture workloads.

Python 3.11+ standard library only. Shell runners are copied to temporary trees
where fixture/harness entry points are replaced with recording stubs. This is
wiring coverage, not evidence that any integration suite has passed.
"""
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import tomllib
import unittest

ROOT = Path(__file__).resolve().parents[2]
SECURITY_CASES = {
    "oauth_bearer_authentication",
    "status_authorization_matrix",
    "authorized_fetch_read_routes_require_signatures",
    "browser_recovery_fences",
    "browser_reauthentication_limits",
}
BASE_CASES = {
    "core_rest_serializers",
    "rest_protocol_contracts",
    "federation_discovery",
    "actor_media_root_url",
    "quote_lifecycle",
}


def job_blocks(workflow):
    """Read this workflow's two-space job blocks, not arbitrary YAML."""
    jobs = workflow.split("\njobs:\n", 1)[1]
    return dict(re.findall(r"^  ([\w-]+):\n(.*?)(?=^  [\w-]+:\n|\Z)",
                           jobs, re.MULTILINE | re.DOTALL))


class ConfigurationContracts(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tasks = tomllib.loads((ROOT / "mise.toml").read_text())["tasks"]
        cls.workflow = (ROOT / ".github/workflows/ci.yml").read_text()
        cls.jobs = job_blocks(cls.workflow)

    def test_check_is_bounded_and_selects_profile_and_harness_gates(self):
        dependencies = set(self.tasks["check"]["depends"])
        self.assertTrue({"test", "test-default", "test-release", "harness-tests"}
                        <= dependencies, dependencies)
        self.assertFalse(dependencies & {
            "differential-full", "browser-integration", "cutover-integration",
            "peer-public", "peer-privacy", "peer-notes", "peer-profile",
            "peer-interactions",
        })
        self.assertIn("run: mise run check", self.jobs["check"])
        self.assertEqual(self.tasks["test"]["run"],
                         "cargo test --locked --all-targets --all-features")
        self.assertEqual(self.tasks["test-default"]["run"],
                         "cargo test --locked --all-targets")
        self.assertEqual(self.tasks["test-release"]["run"],
                         "cargo test --locked --release --all-targets --all-features")
        self.assertEqual(self.tasks["harness-tests"]["run"], "tools/check-harnesses")
        self.assertNotIn("all Rust tests", self.tasks["test"]["description"])
        self.assertNotIn("all local and CI checks", self.tasks["check"]["description"])

    def test_required_fixture_matrix_includes_http_startup_and_preflight(self):
        job = self.jobs["postgres-integration"]
        self.assertNotRegex(job, r"(?m)^    if:")
        self.assertIn("run: mise run ${{ matrix.task }}", job)
        for task in ("mastodon-schema-integration", "operational-schema-integration",
                     "startup-integration", "preflight-integration"):
            with self.subTest(task=task):
                self.assertRegex(job, rf"(?m)^          - {task}$")
        for task, command in (("startup-integration", "startup-test"),
                              ("preflight-integration", "preflight-test")):
            self.assertEqual(self.tasks[task]["run"], f"tools/mastodon-fixture {command}")
        # Parent's schema aggregate owns the focused HTTP selector inventory.
        self.assertEqual(self.tasks["mastodon-schema-integration"]["run"],
                         "tools/mastodon-fixture schema-read-test")

    def test_required_differential_and_explicit_full_task(self):
        self.assertIn("run: mise run differential-ci", self.jobs["differential"])
        self.assertNotRegex(self.jobs["differential"], r"(?m)^    if:")
        self.assertIn(self.tasks["differential-ci"]["run"],
                      ("tools/ci-differential", "tools/ci-differential required"))
        self.assertEqual(self.tasks["differential-full"]["run"],
                         "tools/ci-differential full")

    def test_broader_fixture_lanes_are_bounded_and_not_push_or_pr_jobs(self):
        self.assertRegex(self.workflow, r"(?m)^  workflow_dispatch:")
        self.assertRegex(self.workflow, r"(?m)^  schedule:")
        job = self.jobs["extended-integration"]
        self.assertIn("runs-on: ubuntu-latest", job)
        self.assertRegex(job, r"(?m)^    timeout-minutes: [1-9][0-9]*$")
        self.assertRegex(job, r"(?m)^      max-parallel: 1$")
        self.assertIn("if: github.event_name == 'schedule' || github.event_name == 'workflow_dispatch'", job)
        self.assertIn("run: mise run ${{ matrix.task }}", job)
        for task in ("differential-full", "browser-integration", "cutover-integration"):
            self.assertRegex(job, rf"(?m)^          - {task}$")

    def test_peer_commands_preserve_runner_guards_and_stay_opt_in(self):
        for scenario in ("public", "privacy", "notes", "profile", "interactions"):
            with self.subTest(scenario=scenario):
                self.assertEqual(self.tasks[f"peer-{scenario}"]["run"],
                                 f"RUSTODON_PEER_SMOKE=1 tools/federation-peer-smoke {scenario}")
                self.assertIn("Manual", self.tasks[f"peer-{scenario}"]["description"])
        # Do not schedule the opt-in peer runner on production or self-hosted
        # runners. Execution requires a separate, explicit authorization.
        self.assertNotIn("self-hosted", self.workflow)
        self.assertNotIn("tools/federation-peer-smoke", self.workflow)
        self.assertNotRegex(self.workflow, r"mise run peer-")


class OfflineDispatchContracts(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="rustodon-ci-contracts-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        (self.root / "tools").mkdir()
        self.log = self.root / "calls"
        # Do not inherit instance credentials, fixture paths or routing overrides.
        self.env = {"PATH": os.defpath, "HOME": str(self.root),
                    "CALL_LOG": str(self.log), "PYTHONDONTWRITEBYTECODE": "1"}

    def copy_runner(self, name):
        source = ROOT / "tools" / name
        self.assertTrue(source.is_file(), f"missing automation runner: {source}")
        destination = self.root / "tools" / name
        shutil.copy2(source, destination)
        return destination

    def run_runner(self, runner, *args, fail=""):
        result = subprocess.run(
            [str(runner), *args], cwd=self.root,
            env=self.env | {"FAIL_CASE": fail}, text=True, capture_output=True,
            timeout=15, check=False,
        )
        calls = self.log.read_text().splitlines() if self.log.exists() else []
        return result, calls

    def differential_runner(self):
        runner = self.copy_runner("ci-differential")
        fixture = self.root / "tools/mastodon-fixture"
        fixture.write_text('''#!/bin/sh
set -eu
[ "$1" = differential-test ] || exit 81
[ "$#" -le 2 ] || exit 82
printf '%s|%s\\n' "${2:-ALL}" "${RUSTODON_DIFFERENTIAL_ACTOR_MEDIA_ROOT:-default}" >> "$CALL_LOG"
[ "${2:-ALL}" != "$FAIL_CASE" ] || exit 37
''')
        fixture.chmod(0o755)
        return runner

    def test_default_and_required_differential_select_security_cases(self):
        runner = self.differential_runner()
        for args in ((), ("required",)):
            with self.subTest(args=args):
                self.log.unlink(missing_ok=True)
                result, calls = self.run_runner(runner, *args)
                self.assertEqual(result.returncode, 0, result.stderr)
                selected = {call.split("|", 1)[0] for call in calls}
                self.assertTrue(SECURITY_CASES | BASE_CASES <= selected, calls)
                self.assertNotIn("ALL", selected)
                self.assertIn("actor_media_root_url|relative", calls)
                self.assertIn("actor_media_root_url|default", calls)
                self.assertEqual(len(calls), len(set(calls)), calls)

    def test_full_differential_selects_all_and_relative_media_variant(self):
        result, calls = self.run_runner(self.differential_runner(), "full")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(calls, ["ALL|default", "actor_media_root_url|relative"])

    def test_invalid_lane_and_extra_arguments_fail_before_fixture_calls(self):
        runner = self.differential_runner()
        for args in (("typo",), ("required", "extra"), ("full", "extra")):
            with self.subTest(args=args):
                self.log.unlink(missing_ok=True)
                result, calls = self.run_runner(runner, *args)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(calls, [], result.stderr)

    def test_differential_failure_stops_remaining_cases(self):
        runner = self.differential_runner()
        result, calls = self.run_runner(runner)
        self.assertEqual(result.returncode, 0, result.stderr)
        first = calls[0].split("|", 1)[0]
        self.log.unlink()
        result, failed_calls = self.run_runner(runner, fail=first)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(failed_calls, calls[:1])

    def test_harness_aggregate_includes_offline_tests_and_propagates_failure(self):
        runner = self.copy_runner("check-harnesses")
        paths = sorted(path.relative_to(ROOT).as_posix()
                       for path in (ROOT / "tools/tests").iterdir()
                       if path.name.endswith(("-test", "-test.py")))
        paths += ["tools/federation-pleroma-build-test.py",
                  "tools/peer-tests/test_tls_proxy.py"]
        for path in paths:
            stub = self.root / path
            stub.parent.mkdir(parents=True, exist_ok=True)
            if path.endswith(".py"):
                stub.write_text(
                    "import os, unittest\n"
                    "class RecordedHarness(unittest.TestCase):\n"
                    "    def test_record(self):\n"
                    f"        with open(os.environ['CALL_LOG'], 'a') as log: log.write({path!r} + '\\n')\n"
                    f"        self.assertNotEqual(os.environ['FAIL_CASE'], {path!r})\n"
                    "if __name__ == '__main__': unittest.main()\n"
                )
            else:
                stub.write_text(f'#!/bin/sh\nprintf "%s\\n" "{path}" >> "$CALL_LOG"\n'
                                f'[ "$FAIL_CASE" != "{path}" ] || exit 37\n')
            stub.chmod(0o755)
        result, calls = self.run_runner(runner)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertCountEqual(calls, paths)
        self.log.unlink()
        result, failed_calls = self.run_runner(runner, fail=calls[0])
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(failed_calls, calls[:1])


if __name__ == "__main__":
    unittest.main()
