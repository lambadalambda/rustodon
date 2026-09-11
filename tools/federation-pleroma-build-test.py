#!/usr/bin/env python3
"""Offline guard tests; run only on Secunda."""
import importlib.machinery
import importlib.util
from pathlib import Path
import tempfile
import unittest

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
        recipe = peer.pinned_recipe(original, builder, runtime)
        self.assertEqual(
            [line for line in recipe.splitlines() if line.startswith("FROM ")],
            [f"FROM {builder} AS build", f"FROM {runtime}"],
        )
        self.assertIn("RUN mix release --path release\n", recipe)
        self.assertIn(peer.LOCK_SHA256, recipe)
        self.assertIn("mix hex.info", recipe)
        self.assertIn("rebar3", recipe)
        self.assertEqual(recipe.count("apk info -vv"), 2)
        with self.assertRaises(ValueError):
            peer.pinned_recipe("unexpected Dockerfile", builder, runtime)


if __name__ == "__main__":
    unittest.main()
