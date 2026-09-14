#!/usr/bin/env python3
"""Offline cutover rollback wiring/SQL coverage; no fixture workload.

Runs the actual auth snapshot/restore shell blocks with a local SQLite-backed
psql double. Only PostgreSQL format(%s/%L) and setval are emulated; this is not
PostgreSQL integration evidence. The table columns come from the pinned fixture.
No containers, services, network access, or browser execution.
"""
import json
import os
from pathlib import Path
import re
import sqlite3
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
FIXTURE = ROOT / "tools/mastodon-fixture"
SCHEMA = ROOT / "fixtures/mastodon/v4.6.5/database.sql"
SELF = Path(__file__).resolve()


def statements(text):
    pending = ""
    for char in text:
        pending += char
        if char == ";" and sqlite3.complete_statement(pending):
            yield pending
            pending = ""
    if pending.strip():
        yield pending


def connection(path):
    database = sqlite3.connect(":memory:")
    database.execute("ATTACH DATABASE ? AS public", (str(path),))

    def sql_format(template, *values):
        values = iter(values)

        def replace(match):
            value = next(values)
            if match[0] == "%s":
                return "" if value is None else str(value)
            return "NULL" if value is None else "'" + str(value).replace("'", "''") + "'"

        return re.sub(r"%[sL]", replace, template)

    def setval(name, value, called):
        if name not in {f"public.{table}_id_seq" for table in (
                "web_settings", "login_activities", "oauth_access_tokens", "session_activations")}:
            raise ValueError("unexpected sequence write")
        if called not in ("t", "f"):
            raise ValueError("sequence is_called must be preserved, not inferred")
        database.execute(f"UPDATE {name} SET last_value = ?, is_called = ?", (value, called))
        return value

    database.create_function("format", -1, sql_format)
    database.create_function("setval", 3, setval)
    return database


def psql_double():
    args = sys.argv[2:]
    if "ON_ERROR_STOP=1" not in args:
        raise AssertionError("snapshot/restore must fail closed on SQL errors")
    sql = args[args.index("--command") + 1] if "--command" in args else sys.stdin.read()
    with open(os.environ["SQL_LOG"], "a") as log:
        log.write(json.dumps(sql) + "\n")
    failure = os.environ.get("FAIL_SQL", "")
    commands = list(statements(sql))
    with connection(os.environ["TEST_DATABASE"]) as database:
        for index, statement in enumerate(commands):
            if failure and failure in statement:
                return 33
            rows = database.execute(statement).fetchall()
            # PostgreSQL 14 psql -c emits only the last result; stdin emits each.
            if "--command" not in args or index == len(commands) - 1:
                for row in rows:
                    print("|".join("" if value is None else str(value) for value in row))
    return 0


class BrowserSettingsRollback(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="rustodon-browser-settings-rollback-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        (self.root / "target").mkdir()
        self.sibling = self.root / "target/unrelated.sql"
        self.sibling.write_text("preserve")
        self.database_path = self.root / "public.sqlite"
        self.database = connection(self.database_path)
        self.addCleanup(self.database.close)
        schema = SCHEMA.read_text().split("CREATE TABLE public.web_settings (", 1)[1].split("\n);", 1)[0]
        schema = schema.replace("timestamp without time zone", "TEXT").replace("bigint", "INTEGER")
        self.database.execute("CREATE TABLE public.web_settings (" + schema + "\n)")
        self.database.execute("CREATE UNIQUE INDEX public.web_settings_user ON web_settings(user_id)")
        self.database.execute("CREATE TABLE public.users (id INTEGER, current_sign_in_at TEXT, "
                              "last_sign_in_at TEXT, sign_in_count INTEGER, consumed_timestep INTEGER, "
                              "updated_at TEXT, otp_backup_codes TEXT)")
        self.database.execute("INSERT INTO public.users VALUES (101, NULL, NULL, 0, NULL, 'before', NULL)")
        for table, owner in [("login_activities", "user_id"), ("session_activations", "user_id"),
                             ("oauth_access_tokens", "resource_owner_id")]:
            self.database.execute(f"CREATE TABLE public.{table} (id INTEGER, {owner} INTEGER)")
            self.database.execute(f"INSERT INTO public.{table} VALUES (10, 101), (90, 202)")
        for table in ("web_settings", "login_activities", "oauth_access_tokens", "session_activations"):
            self.database.execute(f"CREATE TABLE public.{table}_id_seq (last_value INTEGER, is_called TEXT)")
            self.database.execute(f"INSERT INTO public.{table}_id_seq VALUES (1, 'f')")
        self.database.commit()
        source = FIXTURE.read_text()
        start = source.index('  case "${RUSTODON_BROWSER_AUTH:-false}" in', source.index("cutover_test() {"))
        end = source.index('  case "${RUSTODON_BROWSER_SMOKE:-false}" in', start)
        self.snapshot = source[start:end]
        start = source.index('  if [ -n "$cutover_browser_user_restore" ]; then', end)
        end = source.index('  RUSTODON_SMOKE_DOMAIN="$LOCAL_DOMAIN"', start)
        self.restore = source[start:end]
        self.env = {"PATH": os.defpath, "HOME": str(self.root), "PROBE_ROOT": str(self.root),
                    "TEST_DATABASE": str(self.database_path), "SQL_LOG": str(self.root / "sql.log"),
                    "PYTHONDONTWRITEBYTECODE": "1"}

    def shell(self, body, **environment):
        prelude = '''
MASTODON_FIXTURE_LIBRARY=1
. "$1"
ROOT=$PROBE_ROOT
PG_CONTAINER=offline-fixture
RUSTODON_BROWSER_AUTH=true
cutover_browser_user_restore=''
# Capture script arguments before function arguments shadow them.
test_python=$2
test_program=$3
run_podman() { "$test_python" "$test_program" --psql "$@"; }
'''
        return subprocess.run(["sh", "-c", prelude + body, "test", str(FIXTURE), sys.executable, str(SELF)],
                              env=dict(self.env, **environment), capture_output=True, text=True, timeout=8)

    def capture(self, **environment):
        return self.shell(self.snapshot + '''
printf '%s\n' "$cutover_browser_user_restore" > "$ROOT/restore-path"
printf '%s\n' "$cutover_browser_login_activity_max" "$cutover_browser_session_activation_max" \
  "$cutover_browser_token_max" > "$ROOT/auth-baselines"
''', **environment)

    def replay(self, **environment):
        body = '''
cutover_browser_user_restore=$(cat "$ROOT/restore-path")
{
  read -r cutover_browser_login_activity_max
  read -r cutover_browser_session_activation_max
  read -r cutover_browser_token_max
} < "$ROOT/auth-baselines"
'''
        return self.shell(body + self.restore + '\nprintf restored > "$ROOT/restored"\n', **environment)

    def row(self, user_id):
        return self.database.execute("SELECT * FROM public.web_settings WHERE user_id = ?", (user_id,)).fetchall()

    def sequence(self):
        return self.database.execute("SELECT last_value, is_called FROM public.web_settings_id_seq").fetchone()

    def seed(self, existing, sequence, data=' { "note": "Alice\'s settings", "escaped": "\\n", "emoji": "🐈" } '):
        self.database.execute("DELETE FROM public.web_settings")
        self.database.execute("UPDATE public.web_settings_id_seq SET last_value = ?, is_called = ?", sequence)
        self.database.execute("INSERT INTO public.web_settings VALUES (90, 'other-created', '{}', 'other-updated', 202)")
        if existing:
            self.database.execute("INSERT INTO public.web_settings VALUES (17, '2026-07-01 01:02:03.123456', ?, "
                                  "'2026-07-02 04:05:06.654321', 101)", (data,))
        self.database.commit()

    def browser_write(self):
        self.database.execute("DELETE FROM public.web_settings WHERE user_id = 101")
        self.database.execute("INSERT INTO public.web_settings VALUES (18, 'browser-created', "
                              "'{\"home\":{\"shows\":{\"reblog\":false}}}', 'browser-updated', 101)")
        self.database.execute("UPDATE public.web_settings_id_seq SET last_value = 19, is_called = 't'")
        self.database.commit()

    def test_absent_existing_and_null_settings_restore_all_columns_and_exact_sequence(self):
        for existing, sequence, data in [(False, (1, "f"), None), (False, (42, "t"), None),
                                        (True, (23, "t"), " { \"quote\": \"Alice's\", \"line\": \"\\n\" } "),
                                        (True, (23, "f"), None)]:
            with self.subTest(existing=existing, sequence=sequence, data=data):
                self.seed(existing, sequence, data)
                before = self.row(101)
                self.assertEqual(self.capture().returncode, 0)
                self.browser_write()
                result = self.replay()
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(self.row(101), before)
                self.assertEqual(self.sequence(), sequence)
                self.assertEqual(self.row(202), [(90, "other-created", "{}", "other-updated", 202)])
                self.assertFalse(Path((self.root / "restore-path").read_text().strip()).exists())
                self.assertEqual(self.sibling.read_text(), "preserve")

    def test_restore_does_not_erase_or_undo_other_users_changes(self):
        self.seed(True, (23, "t"))
        before = self.row(101)
        self.assertEqual(self.capture().returncode, 0)
        self.browser_write()
        self.database.execute("UPDATE public.web_settings SET data = 'unrelated-change' WHERE user_id = 202")
        self.database.execute("INSERT INTO public.web_settings VALUES (91, 'new', '{}', 'new', 303)")
        self.database.commit()
        others = self.row(202) + self.row(303)
        result = self.replay()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.row(101), before)
        self.assertEqual(self.row(202) + self.row(303), others)

    def test_snapshot_sequence_failure_stops_before_browser_can_run(self):
        self.seed(False, (1, "f"))
        result = self.capture(FAIL_SQL="web_settings_id_seq")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.root / "restore-path").exists())
        self.assertEqual(self.row(101), [])
        self.assertEqual(self.sequence(), (1, "f"))

    def test_restore_sequence_failure_is_not_success_and_keeps_rollback_file(self):
        self.seed(True, (23, "f"))
        before = self.row(101)
        self.assertEqual(self.capture().returncode, 0)
        self.browser_write()
        result = self.replay(FAIL_SQL="web_settings_id_seq")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.root / "restored").exists())
        self.assertEqual(self.row(101), before, "row replay precedes the failing setval")
        self.assertEqual(self.sequence(), (19, "t"))
        # Block-local failure must not report cleanup success. The actual EXIT
        # trap removes this private file, as the separate abort test verifies.
        self.assertTrue(Path((self.root / "restore-path").read_text().strip()).exists())
        self.assertEqual(self.sibling.read_text(), "preserve")

    def test_psql_double_matches_command_versus_stdin_result_visibility(self):
        for request, expected in [
                ('--command "SELECT 11; SELECT 22;"', "22\n"),
                ("<<'SQL'\nSELECT 11; SELECT 22;\nSQL", "11\n22\n")]:
            with self.subTest(request=request):
                result = self.shell("run_podman exec offline-fixture psql --set ON_ERROR_STOP=1 " + request)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stdout, expected)

    def test_abort_removes_only_owned_restore_file_even_when_artifacts_retained(self):
        self.seed(False, (1, "f"))
        self.assertEqual(self.capture().returncode, 0)
        result = self.shell('''
cutover_browser_user_restore=$(cat "$ROOT/restore-path")
CUTOVER_KEEP_ARTIFACTS=true
cleanup_containers() { :; }
trap cutover_abort EXIT
exit 7
''')
        self.assertEqual(result.returncode, 7, result.stderr)
        self.assertFalse(Path((self.root / "restore-path").read_text().strip()).exists())
        self.assertEqual(self.sibling.read_text(), "preserve")

    def test_full_public_data_gate_and_authentication_scope_remain_intact(self):
        source = FIXTURE.read_text()
        dump = source.split("dump_cutover_public_state() {", 1)[1].split("\n}\n", 1)[0]
        self.assertNotIn("web_settings", dump, "do not special-case settings out of the full dump")
        self.assertIn('cmp -s "${cutover_before_prefix}-data" "${cutover_after_prefix}-data"', source)
        self.assertIn("die 'cutover changed Mastodon public data'", source)
        self.assertIn("public.web_settings", self.snapshot)
        result = self.shell('RUSTODON_BROWSER_AUTH=false\n' + self.snapshot)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse((self.root / "sql.log").exists(), "unauthenticated smoke must not snapshot settings")


if __name__ == "__main__":
    if sys.argv[1:2] == ["--psql"]:
        sys.exit(psql_double())
    unittest.main()
