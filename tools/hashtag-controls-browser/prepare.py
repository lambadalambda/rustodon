#!/usr/bin/env python3
"""Fixed task adaptation of remote-browser; source/application stay immutable."""
from pathlib import Path
import sys

REV = 'bd01acae2bc4e1b8a75bd95648e216535c790330'
NAME = 'hashtag-browser-bd01aca'


def once(text, old, new):
    if text.count(old) != 1:
        raise ValueError('base harness drift: ' + old[:80])
    return text.replace(old, new, 1)


def prepare(root, out, mode='browser'):
    if mode not in ('browser', 'differential'):
        raise ValueError('unknown bounded mode')
    name_prefix = NAME if mode == 'browser' else 'hashtag-differential-bd01aca'
    out.mkdir(parents=True, exist_ok=True)
    for name in ('run.sh', 'setup.sh', 'cleanup.sh', 'browser', 'forward.py', 'ready.py'):
        text = (root / 'tools/remote-browser' / name).read_text()
        text = text.replace('8b2f49e42a7cddd75b9d2dce4be24cfcd388099c', REV)
        text = text.replace('remote-browser-8b2f49e-r2', name_prefix)
        text = text.replace('tools/remote-browser/', 'tools/hashtag-runtime/')
        if name == 'run.sh':
            text = '\n'.join(line for line in text.split('\n') if not line.startswith(('run source ', 'run pull ')))
            start = text.index('podman exec "$P-browser-alice" sh /harness/')
            end = text.index('podman image inspect', start)
            text = text[:start] + '''timeout -k 10 600 podman exec "$P-browser-alice" python3 /harness/tools/hashtag-controls-browser/ui.py > evidence/accept.log 2>&1
''' + text[end:]
        elif name == 'setup.sh':
            text = '\n'.join(line for line in text.split('\n') if not line.startswith(('RUSTODON_TEST_PEER_ORIGINS=', 'RUSTODON_TEST_PEER_CA=')))
            start = text.index('openssl req -x509')
            end = text.index('for mode in web worker; do', start)
            text = text[:start] + '''podman exec -i "$PG" psql -U postgres -d remote_browser -v ON_ERROR_STOP=1 < harness/tools/hashtag-controls-browser/seed.sql > evidence/seed.log
''' + text[end:]
            text = once(text, 'for mode in web worker; do', 'for mode in web; do')
            text = once(text, '--memory=1g --memory-swap=1g', '--memory=2g --memory-swap=2g')
        elif name == 'cleanup.sh':
            text = once(text, '        podman logs', '        podman inspect --format \'{{.Name}} exit={{.State.ExitCode}} oom={{.State.OOMKilled}} status={{.State.Status}}\' "$P-$n-alice" >> evidence/container-state.txt\n        podman logs')
        elif name == 'ready.py':
            text = once(text, ",\n              (19443, 'remote.fixture.invalid', root / 'remote.pem', '/not-media', b'404')", '')
        if mode == 'differential':
            if name == 'run.sh':
                text = text[:text.index('run() {')] + '''timeout -k 15 420 sh harness/tools/hashtag-controls-browser/differential.sh > evidence/differential.log 2>&1
printf '%s\\n' 'PASS: actual pinned Rails differential' > evidence/result.txt
'''
                text = text.replace('build migrate pg web worker pull source tls forward browser;', 'build migrate pg web worker pull source tls forward browser rails redis compare;')
            elif name == 'setup.sh':
                text = once(text, '/seed.sql', '/differential-seed.sql')
            elif name == 'cleanup.sh':
                text = once(text, 'for n in browser forward', 'for n in compare rails redis browser forward')
                text = once(text, 'rm -rf run-fixture media target', 'rm -rf run-fixture media rails-media target')
        (out / name).write_text(text)
    if mode == 'differential':
        # Reuse the exact existing Rails launch environment/image rather than
        # inventing an oracle. No changes to reference source or application.
        fixture = (root / 'tools/mastodon-fixture').read_text()
        constants = fixture.split('SCRIPT_DIR=', 1)[0]
        web = fixture.split('start_differential_web() {', 1)[1].split('\n}\n', 1)[0]
        web = 'start_differential_web() {' + web + '\n}\n'
        web = once(web, 'WEB_CONTAINER="rustodon-differential-v4-6-5-web-$$"', 'WEB_CONTAINER="$P-rails-alice"')
        web = once(web, '    --publish 127.0.0.1::3000 \\\n', '')
        web = once(web, '  run_podman run -d \\\n', '  run_podman run -d --pull=never --cpus=2 --memory=2g --memory-swap=2g --pids-limit=256 --timeout=600 \\\n')
        web = once(web, 'curl -fsS --noproxy', 'curl --max-time 2 -fsS --noproxy')
        (out / 'rails-functions.sh').write_text(constants + '\n' + web)


if __name__ == '__main__':
    prepare(Path(sys.argv[1]), Path(sys.argv[2]), sys.argv[3] if len(sys.argv) > 3 else 'browser')
