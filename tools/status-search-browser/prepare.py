#!/usr/bin/env python3
"""Materialize a fixed, task-only adaptation; never edit the media harness."""
from pathlib import Path
import sys

REV = 'c3497f5cdf71678c70542689e079ac589de253a1'
OLD_REV = '8b2f49e42a7cddd75b9d2dce4be24cfcd388099c'
NAME = 'status-search-browser-c3497f5'
# Owner-only observation, never used as runtime credentials or to insert results.
PRIVACY_SQL = "SELECT json_build_object('mentions',(SELECT count(*) FROM mentions),'hidden',(SELECT md5(row_to_json(s)::text) FROM statuses s WHERE id=-312),'private_count',(SELECT count(*) FROM statuses WHERE uri='https://remote.fixture.invalid/notes/status-search-c3497f5-private'));"


def replace_once(text, old, new):
    if text.count(old) != 1:
        raise ValueError('media harness drift at: ' + old[:80])
    return text.replace(old, new, 1)


def prepare(root, out):
    out.mkdir(parents=True, exist_ok=True)
    for name in ('run.sh', 'setup.sh', 'cleanup.sh', 'browser', 'forward.py', 'ready.py'):
        text = (root / 'tools/remote-browser' / name).read_text()
        text = text.replace(OLD_REV, REV).replace('remote-browser-8b2f49e-r2', NAME)
        text = text.replace('tools/remote-browser/', 'tools/status-search-runtime/')
        if name == 'run.sh':
            text = replace_once(text, 'python3 /harness/tools/status-search-runtime/fixture.py serve',
                                'python3 /harness/tools/status-search-browser/source.py')
            text = '\n'.join(line for line in text.split('\n') if not line.startswith('run pull '))
            start = text.index('podman exec "$P-browser-alice" sh /harness/')
            end = text.index('podman image inspect', start)
            text = text[:start] + '''timeout -k 10 600 podman exec "$P-browser-alice" python3 /harness/tools/status-search-browser/ui.py > evidence/accept.log 2>&1
podman exec "$P-pg-alice" psql -U postgres -d remote_browser -At -v ON_ERROR_STOP=1 -c "SELECT json_build_object('public_count',(SELECT count(*) FROM statuses WHERE uri='https://remote.fixture.invalid/notes/status-search-c3497f5-public'),'private_count',(SELECT count(*) FROM statuses WHERE uri='https://remote.fixture.invalid/notes/status-search-c3497f5-private'),'mentions',(SELECT count(*) FROM mentions WHERE status_id IN (SELECT id FROM statuses WHERE uri LIKE 'https://remote.fixture.invalid/notes/status-search-c3497f5-%')));" > evidence/persistence.json
python3 - <<'CHECK'
import json
from pathlib import Path
assert json.loads(Path('evidence/persistence.json').read_text()) == {'public_count': 1, 'private_count': 0, 'mentions': 0}
CHECK
''' + text[end:]
            text = replace_once(text, "python3 - <<'CHECK'", 'podman exec "$P-pg-alice" psql -U postgres -d remote_browser -At -v ON_ERROR_STOP=1 -c "' + PRIVACY_SQL + '" > evidence/privacy-after.json\n' + "python3 - <<'CHECK'")
            text = replace_once(text, '\nCHECK\n', "\nassert json.loads(Path('evidence/privacy-before.json').read_text()) == json.loads(Path('evidence/privacy-after.json').read_text())\nCHECK\n")
        elif name == 'setup.sh':
            text = replace_once(text, 'for mode in web worker; do', 'for mode in web; do')
            marker = 'for mode in web; do'
            extra = '''# Only the public fixture key is exposed to the source verifier.
podman exec "$PG" psql -U postgres -d remote_browser -At -c "SELECT public_key FROM accounts WHERE id=-99" > run-fixture/searcher.pub
podman exec "$PG" psql -U postgres -d remote_browser -At -c "SELECT max(version) FROM rustodon.schema_migrations" > evidence/migration-version.txt
test "$(cat evidence/migration-version.txt)" = 6
'''
            extra += 'podman exec "$PG" psql -U postgres -d remote_browser -At -v ON_ERROR_STOP=1 -c "' + PRIVACY_SQL + '" > evidence/privacy-before.json\n'
            extra += '''podman exec "$PG" psql -U postgres -d remote_browser -At -v ON_ERROR_STOP=1 -c "SELECT count(*) FROM statuses WHERE uri LIKE 'https://remote.fixture.invalid/notes/status-search-c3497f5-%'" > evidence/uncached-before.txt
test "$(cat evidence/uncached-before.txt)" = 0
'''
            text = replace_once(text, marker, extra + marker)
        (out / name).write_text(text)


if __name__ == '__main__':
    prepare(Path(sys.argv[1]), Path(sys.argv[2]))
