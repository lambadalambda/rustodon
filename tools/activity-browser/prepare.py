#!/usr/bin/env python3
"""Fixed activity acceptance adaptation; immutable source, existing browser stack."""
from pathlib import Path
import sys

REV = '10ddf66caf1fe8714f7c41ac4a1a0b8ceaf7fe34'
NAME = 'activity-browser-10ddf66'


def once(text, old, new):
    if text.count(old) != 1:
        raise ValueError('base harness drift: ' + old[:80])
    return text.replace(old, new, 1)


def prepare(root, out):
    out.mkdir(parents=True, exist_ok=True)
    for name in ('run.sh', 'setup.sh', 'cleanup.sh', 'browser', 'forward.py', 'ready.py'):
        text = (root/'tools/remote-browser'/name).read_text()
        text = text.replace('8b2f49e42a7cddd75b9d2dce4be24cfcd388099c', REV)
        text = text.replace('remote-browser-8b2f49e-r2', NAME)
        text = text.replace('tools/remote-browser/', 'tools/activity-runtime/')
        if name == 'run.sh':
            text = once(text, '--features test-support ', '')
            text = once(text, '-v "$W/source:/workspace:ro" -v /srv/workspaces/', '-v "$W/source:/workspace:ro" -v "$W/reference:/workspace/target/mastodon-v4.6.5:ro" -v /srv/workspaces/')
            text = once(text, 'sha256sum /evidence/rustodon', 'sha256sum /evidence/rustodon && cargo test --locked --offline --lib activity:: -- --test-threads=1 && cargo test --locked --offline --test pinned_source_contracts pinned_daily_activity_records_and_interactive_tracking_contract -- --ignored --exact --test-threads=1')

            text = '\n'.join(line for line in text.split('\n') if not line.startswith(('run source ', 'run pull ')))
            start = text.index('podman exec "$P-browser-alice" sh /harness/')
            end = text.index('podman image inspect', start)
            text = text[:start] + 'timeout -k 10 600 sh harness/tools/activity-browser/stages.sh > evidence/accept.log 2>&1\n' + text[end:]
        elif name == 'setup.sh':
            text = '\n'.join(line for line in text.split('\n') if not line.startswith(('RUSTODON_TEST_PEER_ORIGINS=', 'RUSTODON_TEST_PEER_CA=')))
            start = text.index('openssl req -x509')
            end = text.index('for mode in web worker; do', start)
            text = text[:start] + '''# Make the existing confirmed fixture account eligible for a real returning login.
podman exec -i "$PG" psql -U postgres -d remote_browser -v ON_ERROR_STOP=1 > evidence/baseline.sql.txt <<'SQL'
UPDATE users SET current_sign_in_at=NULL WHERE account_id=116844606259201001;
SELECT count(*) AS empty_members FROM rustodon.activity_members;
SELECT * FROM rustodon.schema_migrations ORDER BY version;
SQL
''' + text[end:]
            text = once(text, 'for mode in web worker; do', 'for mode in web; do')
        elif name == 'ready.py':
            text = once(text, ",\n              (19443, 'remote.fixture.invalid', root / 'remote.pem', '/not-media', b'404')", '')
        elif name == 'cleanup.sh':
            text = once(text, '        podman logs', '        podman inspect --format \'{{.Name}} exit={{.State.ExitCode}} oom={{.State.OOMKilled}}\' "$P-$n-alice" >> evidence/container-state.txt\n        podman logs')
        (out/name).write_text(text)


if __name__ == '__main__':
    prepare(Path(sys.argv[1]), Path(sys.argv[2]))
