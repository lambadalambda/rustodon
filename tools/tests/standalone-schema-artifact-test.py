#!/usr/bin/env python3
"""Offline contracts for the standalone PostgreSQL schema artifact."""

from __future__ import annotations

import hashlib
import json
import pathlib
import re
import subprocess
import sys
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
TOOL = ROOT / "tools" / "standalone-schema-artifact"
ARTIFACT_DIR = ROOT / "migrations" / "mastodon" / "v4.6.5"
SCHEMA = ARTIFACT_DIR / "public-schema.sql"
MANIFEST = ARTIFACT_DIR / "manifest.json"


class StandaloneSchemaArtifactTest(unittest.TestCase):
    def test_checked_artifact_is_safe_and_matches_manifest(self) -> None:
        subprocess.run([str(TOOL), "verify", str(ARTIFACT_DIR)], check=True)

        manifest = json.loads(MANIFEST.read_text())
        schema = SCHEMA.read_text()
        self.assertEqual(manifest["mastodon_revision"], "1440d55b139e39ec722c2a3db7f60b66cd889048")
        self.assertEqual(manifest["postgres_version"], "14.23")
        self.assertEqual(manifest["schema_sha256"], hashlib.sha256(SCHEMA.read_bytes()).hexdigest())
        self.assertEqual(schema.count("__RUSTODON_TIMESTAMP_ID_SALT__"), 1)
        self.assertIn("CREATE TABLE public.accounts", schema)
        self.assertIn("CREATE MATERIALIZED VIEW public.instances", schema)
        self.assertNotRegex(schema, re.compile(r"^\\", re.MULTILINE))
        self.assertNotIn("fixture-v4-6-5.rustodon.invalid", schema)
        self.assertNotIn("fixture-password", schema)

    def test_normalization_rejects_data_statements(self) -> None:
        raw = """-- Dumped from database version 14.23
\\restrict token
--
-- Name: public; Type: SCHEMA; Schema: -; Owner: -
--

CREATE SCHEMA public;


CREATE FUNCTION public.timestamp_id(table_name text) RETURNS bigint LANGUAGE sql AS $$ SELECT length('rustodon-mastodon-v4.6.5-fixture-salt') $$;
COPY public.accounts (id) FROM stdin;
\\unrestrict token
"""
        with tempfile.TemporaryDirectory() as directory:
            source = pathlib.Path(directory) / "raw.sql"
            output = pathlib.Path(directory) / "artifact"
            source.write_text(raw)
            result = subprocess.run(
                [str(TOOL), "normalize", str(source), str(output)],
                text=True,
                capture_output=True,
            )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("data statement", result.stderr)


if __name__ == "__main__":
    unittest.main()
