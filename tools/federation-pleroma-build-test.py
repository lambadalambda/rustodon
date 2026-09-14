#!/usr/bin/env python3
"""Offline guard tests for the repository-local peer image builder."""
import importlib.machinery
import importlib.util
from pathlib import Path
import signal
import subprocess
import tempfile
import unittest
from unittest import mock

loader = importlib.machinery.SourceFileLoader(
    "peer_build", str(Path(__file__).with_name("federation-pleroma-build"))
)
spec = importlib.util.spec_from_loader(loader.name, loader)
peer = importlib.util.module_from_spec(spec)
loader.exec_module(peer)


class BuildGuards(unittest.TestCase):
    def test_archive_rejects_wrong_tree_before_extraction(self):
        with tempfile.TemporaryDirectory() as directory:
            archive = Path(directory) / "source.tar"
            archive.write_bytes(b"not the pinned git archive")
            with self.assertRaisesRegex(ValueError, "archive checksum"):
                peer.verify_archive(archive)

    def test_output_is_strictly_beneath_repository_artifact_root(self):
        expected = Path(peer.__file__).resolve().parent.parent / "target" / "pleroma-peer"
        self.assertEqual(peer.ARTIFACT_ROOT, expected)
        self.assertEqual(peer.resolve_output(expected / "build-next"), expected / "build-next")
        for output in (expected, expected.parent, expected.parent / "other-peer"):
            with self.subTest(output=output), self.assertRaisesRegex(ValueError, "beneath"):
                peer.resolve_output(output)
        with tempfile.TemporaryDirectory() as directory:
            temporary = Path(directory)
            root = temporary / "artifacts"
            outside = temporary / "outside"
            root.mkdir()
            outside.mkdir()
            (root / "escape").symlink_to(outside, target_is_directory=True)
            with self.assertRaisesRegex(ValueError, "beneath"):
                peer.resolve_output(root / "escape" / "build", root)

    def test_cache_requires_immutable_unambiguous_linux_amd64(self):
        repository = "docker.io/library/alpine"
        digest = repository + "@sha256:" + "a" * 64
        image = {"RepoDigests": [digest], "Os": "linux", "Architecture": "amd64"}
        self.assertEqual(peer.cached_digest(image, repository), digest)
        for changes in [
            {"RepoDigests": []},
            {"RepoDigests": [repository + ":3.17.9"]},
            {"RepoDigests": [digest, repository + "@sha256:" + "b" * 64]},
            {"Architecture": "arm64"},
            {"RepoDigests": ["example.org/alpine@sha256:" + "a" * 64]},
        ]:
            with self.subTest(changes=changes), self.assertRaises(ValueError):
                peer.cached_digest(image | changes, repository)

    def test_recipe_has_no_floating_from_and_keeps_release_steps(self):
        original = (
            "FROM ${ELIXIR_IMG}:${ELIXIR_VER}-erlang-${ERLANG_VER}-alpine-${ALPINE_VER} AS build\n"
            "COPY . .\nRUN mix release --path release\n"
            "FROM alpine:${ALPINE_VER}\nUSER pleroma\n"
        )
        builder = "docker.io/hexpm/elixir@sha256:" + "a" * 64
        runtime = "docker.io/library/alpine@sha256:" + "b" * 64
        task_label = peer.task_resources("a" * 32)["label"]
        recipe = peer.pinned_recipe(original, builder, runtime, task_label)
        self.assertEqual(
            [line for line in recipe.splitlines() if line.startswith("FROM ")],
            [f"FROM {builder} AS build", f"FROM {runtime}"],
        )
        self.assertIn("RUN mix release --path release\n", recipe)
        self.assertIn(peer.LOCK_SHA256, recipe)
        self.assertIn("mix hex.info", recipe)
        self.assertIn("rebar3", recipe)
        self.assertEqual(recipe.count("apk info -vv"), 2)
        self.assertEqual(recipe.count("LABEL " + task_label), 2)
        with self.assertRaises(ValueError):
            peer.pinned_recipe("unexpected Dockerfile", builder, runtime, task_label)

    def test_commands_force_local_cache_only_bounded_execution(self):
        output = Path("output")
        resources = peer.task_resources("a" * 32)
        command = peer.build_command(
            output, output / "recipe", output / "source", "date", resources
        )
        self.assertEqual(command[:2], ["podman", "--remote=false"])
        self.assertIn("--pull=never", command)
        self.assertIn("--platform=linux/amd64", command)
        self.assertIn("--jobs=2", command)
        self.assertIn("--memory=4g", command)
        self.assertIn("--pids-limit=512", command)
        self.assertIn("--rm", command)
        self.assertIn("--force-rm", command)
        self.assertNotIn("--cpu-shares", command)
        period = int(command[command.index("--cpu-period") + 1])
        quota = int(command[command.index("--cpu-quota") + 1])
        self.assertEqual((period, quota), (100_000, 200_000))
        self.assertEqual(quota / period, 2)
        self.assertEqual(command[command.index("--label") + 1], resources["label"])
        self.assertEqual(command[command.index("--layer-label") + 1], resources["label"])
        self.assertEqual(command[command.index("--tag") + 1], resources["image_name"])
        self.assertNotIn("pull", command)

        evidence = peer.evidence_command("sha256:" + "c" * 64, resources)
        self.assertEqual(evidence[:2], ["podman", "--remote=false"])
        self.assertIn("--pull=never", evidence)
        self.assertIn("--network=none", evidence)
        self.assertIn("--memory=512m", evidence)
        self.assertIn("--pids-limit=128", evidence)
        self.assertEqual(evidence[evidence.index("--name") + 1], resources["container_name"])
        self.assertEqual(evidence[evidence.index("--label") + 1], resources["label"])

    def test_failure_reconciliation_is_exactly_task_scoped_without_prune(self):
        resources = peer.task_resources("b" * 32)
        commands = peer.reconciliation_commands(resources)
        queries = peer.reconciliation_queries(resources)
        self.assertEqual(commands, [
            peer.podman_command(
                "container", "rm", "--force", "--ignore", resources["container_name"]
            ),
            peer.podman_command(
                "image", "rm", "--force", "--ignore", resources["image_name"]
            ),
        ])
        self.assertEqual(queries, [
            (
                peer.podman_command(
                    "ps", "--all", "--external", "--quiet", "--filter",
                    "label=" + resources["label"],
                ),
                ["container", "rm", "--force", "--storage"],
            ),
            (
                peer.podman_command(
                    "images", "--all", "--quiet", "--filter",
                    "label=" + resources["label"],
                ),
                ["image", "rm", "--force"],
            ),
        ])
        rendered = [" ".join(command) for command in commands]
        rendered += [" ".join(query) for query, _ in queries]
        self.assertFalse(any("prune" in command for command in rendered))
        self.assertFalse(any("--all" in command for command in commands))
        for command in commands + [query for query, _ in queries]:
            self.assertEqual(command[:2], ["podman", "--remote=false"])

        container_id = "c" * 12
        image_id = "d" * 12
        empty = subprocess.CompletedProcess([], 0, "", None)
        with mock.patch.object(
            peer,
            "run_bounded",
            side_effect=[
                subprocess.CalledProcessError(1, commands[0]),
                subprocess.CompletedProcess([], 0, container_id + "\n", None),
                subprocess.CalledProcessError(1, ["podman", "container", "rm"]),
                subprocess.CompletedProcess([], 0, image_id + "\n", None),
                empty,
                empty,
            ],
        ) as run:
            results = peer.reconcile(resources)
        self.assertEqual(run.call_count, 6)
        self.assertEqual(results[0]["status"], "best-effort cleanup failed")
        self.assertEqual(results[2]["status"], "best-effort cleanup failed")
        self.assertEqual(
            run.call_args_list[2].args[0],
            peer.podman_command("container", "rm", "--force", "--storage", container_id),
        )
        self.assertEqual(
            run.call_args_list[4].args[0],
            peer.podman_command("image", "rm", "--force", image_id),
        )

    def test_bounded_subprocess_uses_and_terminates_its_process_group(self):
        process = mock.Mock(pid=1234, returncode=-signal.SIGKILL)
        process.communicate.side_effect = [
            subprocess.TimeoutExpired(["podman"], 1),
            subprocess.TimeoutExpired(["podman"], peer.TERMINATE_TIMEOUT_SECONDS),
            ("bounded tail", None),
        ]
        with (
            mock.patch.object(peer.subprocess, "Popen", return_value=process) as popen,
            mock.patch.object(peer.os, "killpg") as killpg,
            self.assertRaises(subprocess.TimeoutExpired) as raised,
        ):
            peer.run_bounded(["podman"], timeout=1)
        self.assertTrue(popen.call_args.kwargs["start_new_session"])
        self.assertEqual(popen.call_args.kwargs["stderr"], subprocess.STDOUT)
        self.assertEqual(
            killpg.call_args_list,
            [mock.call(1234, signal.SIGTERM), mock.call(1234, signal.SIGKILL)],
        )
        self.assertEqual(raised.exception.output, "bounded tail")

        interrupted = mock.Mock(pid=5678, returncode=-signal.SIGTERM)
        interrupted.communicate.side_effect = [KeyboardInterrupt(), ("partial", None)]
        with (
            mock.patch.object(peer.subprocess, "Popen", return_value=interrupted),
            mock.patch.object(peer.os, "killpg") as killpg,
            self.assertRaises(peer.CommandInterrupted) as raised,
        ):
            peer.run_bounded(["podman", "info"], timeout=1)
        killpg.assert_called_once_with(5678, signal.SIGTERM)
        self.assertEqual(raised.exception.output, "partial")

    def test_failure_provenance_is_sanitized_and_bounded(self):
        secret_command = ["podman", "run", "--token", "not-for-provenance"]
        error = subprocess.CalledProcessError(
            17, secret_command, output="first\n" + "x" * peer.FAILURE_OUTPUT_LIMIT + "\ntail"
        )
        details = peer.failure_details("collect-evidence", secret_command, error)
        self.assertEqual(details["phase"], "collect-evidence")
        self.assertEqual(details["command"], ["podman", "run", "--token", "<redacted>"])
        self.assertEqual(details["return_code"], 17)
        self.assertFalse(details["timed_out"])
        self.assertTrue(details["combined_output_truncated"])
        self.assertLessEqual(
            len(details["combined_output"].encode()), peer.FAILURE_OUTPUT_LIMIT
        )
        self.assertTrue(details["combined_output"].endswith("tail"))

        timeout = subprocess.TimeoutExpired(["podman", "info"], 30, output="partial")
        details = peer.failure_details("inspect-engine", timeout.cmd, timeout)
        self.assertTrue(details["timed_out"])
        self.assertEqual(details["timeout_seconds"], 30)
        self.assertIsNone(details["return_code"])

        timeout = subprocess.TimeoutExpired(secret_command, 30)
        details = peer.failure_details("collect-evidence", secret_command, timeout)
        self.assertEqual(details["combined_output"], "")
        self.assertNotIn("not-for-provenance", str(details))

    def test_build_failure_provenance_reads_bounded_log_without_replacing_it(self):
        with tempfile.TemporaryDirectory() as directory:
            build_log = Path(directory) / "build.log"
            complete_log = "start\n" + "z" * peer.FAILURE_OUTPUT_LIMIT + "\nend"
            build_log.write_text(complete_log)
            error = subprocess.CalledProcessError(3, ["podman", "build"])
            details = peer.failure_details(
                "build-image", error.cmd, error, output_path=build_log
            )
            self.assertEqual(build_log.read_text(), complete_log)
            self.assertTrue(details["combined_output"].endswith("end"))
            self.assertTrue(details["combined_output_truncated"])
            self.assertEqual(details["return_code"], 3)

    def test_subprocess_environment_excludes_remote_connections_and_secrets(self):
        environment = peer.podman_environment({
            "PATH": "/bin",
            "HOME": "/home/test",
            "XDG_RUNTIME_DIR": "/run/user/1",
            "CONTAINER_HOST": "ssh://remote/run/podman.sock",
            "CONTAINER_CONNECTION": "remote",
            "DATABASE_URL": "secret",
        })
        self.assertEqual(environment, {
            "PATH": "/bin",
            "HOME": "/home/test",
            "XDG_RUNTIME_DIR": "/run/user/1",
        })
        self.assertGreater(peer.BUILD_TIMEOUT_SECONDS, peer.EVIDENCE_TIMEOUT_SECONDS)
        self.assertGreater(peer.EVIDENCE_TIMEOUT_SECONDS, peer.INSPECT_TIMEOUT_SECONDS)


if __name__ == "__main__":
    unittest.main()
