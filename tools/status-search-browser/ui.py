#!/usr/bin/env python3
"""Uninterrupted focused controller: genuine input, Posts, click and reload."""
import importlib.util
import json
from pathlib import Path
import subprocess

spec = importlib.util.spec_from_file_location('media_ui', '/harness/tools/remote-browser/ui.py')
ui = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ui)
BASE = ui.BASE
REMOTE = 'https://remote.fixture.invalid'
KNOWN = BASE + '/@alice/116844842188805001'
PUBLIC = REMOTE + '/notes/status-search-c3497f5-public'
PRIVATE = REMOTE + '/notes/status-search-c3497f5-private'
HIDDEN = REMOTE + '/users/carol/statuses/-312'


def ab(*args, stdin=None):
    result = subprocess.run(['sh', '/harness/tools/status-search-runtime/browser', *args], input=stdin, text=True, capture_output=True, timeout=45)
    if result.returncode:
        raise RuntimeError('browser command failed: ' + args[0] + '\n' + result.stderr)
    return result.stdout.strip()


ui.ab = ab
js, wait, save = ui.js, ui.wait, ui.save


def source_count():
    p = Path('/run-fixture/source.jsonl')
    rows = [json.loads(line) for line in p.read_text().splitlines()] if p.exists() else []
    assert all(r['signature_valid'] for r in rows), rows
    return len(rows)


def search(url, label, expected=True):
    # New document clears both Redux cache and the passive observer interval.
    ab('open', BASE + '/home')
    wait('Boolean(document.querySelector("input.search__input"))')
    ab('fill', 'input.search__input', url)
    ab('press', 'Enter')
    wait('window.statusSearchEvidence.some(r=>r.path==="/api/v2/search" && r.params.q===' + json.dumps(url) + ')')
    ab('find', 'role', 'button', 'click', '--name', 'Posts', '--exact')
    wait('window.statusSearchEvidence.some(r=>r.params?.type==="statuses" && r.params.q===' + json.dumps(url) + ')')
    records = js('window.statusSearchEvidence.filter(r=>r.path==="/api/v2/search")')
    matches = [r for r in records if r['params'].get('q') == url]
    assert matches and all(r['code'] == 200 for r in matches), records
    assert all(r['params'].get('resolve') == 'true' and r['params'].get('limit') == '11' and r['params'].get('offset', '0') == '0' for r in matches), records
    assert any('type' not in r['params'] for r in matches), records
    typed = [r for r in matches if r['params'].get('type') == 'statuses'][-1]
    if expected:
        assert len(typed['statuses']) == 1, typed
        status = typed['statuses'][0]
        assert status['url'] == url, status
        wait('Boolean(document.querySelector(".explore__search-results .status"))')
        visible = js('document.querySelector(".explore__search-results").innerText')
        assert ('Public fixture status with local media' if label == 'known' else 'Status search c3497f5 public accepted') in visible, visible
    else:
        assert typed['statuses'] == [] and typed['accounts'] == 0, typed
        wait('document.querySelector(".explore__search-results")?.innerText.includes("No results.")')
        visible = js('document.querySelector(".explore__search-results").innerText')
        assert 'PRIVATE MUST NOT DISPLAY' not in visible and 'Former follower private denial' not in visible
        assert js('document.querySelectorAll(".explore__search-results .status").length') == 0
        status = None
    save(label + '-search', {'requests': matches, 'results_text': visible, 'source_count': source_count()})
    ab('screenshot', '/evidence/' + label + '-posts.png')
    if status:
        # Native status timestamp click invokes the frontend's own permalink routing.
        ab('click', '.explore__search-results .status__relative-time')
        wait('location.pathname.endsWith(' + json.dumps('/' + status['id']) + ')')
        permalink = js('location.origin + location.pathname')
        assert permalink.startswith(BASE + '/@') and permalink.endswith('/' + status['id']), permalink
        save(label + '-navigate', {'permalink': permalink, 'status': status})
        ab('reload')
        wait('window.statusSearchEvidence.some(r=>r.id===' + json.dumps(status['id']) + ' && r.code===200)')
        wait('Boolean(document.querySelector(".detailed-status"))')
        text = js('document.querySelector(".detailed-status").innerText')
        assert ('Public fixture status with local media' if label == 'known' else 'Status search c3497f5 public accepted') in text, text
        assert js('location.origin + location.pathname') == permalink
        save(label + '-reload', {'permalink': permalink, 'requests': js('window.statusSearchEvidence'), 'text': text})
        ab('screenshot', '/evidence/' + label + '-reload.png')


def main():
    # Compose existing socket observer for existing login readiness, plus search observer.
    observer = Path('/run-fixture/search-observe.js')
    observer.write_text(Path('/harness/tools/remote-browser/observe.js').read_text() + '\n' + Path('/harness/tools/status-search-browser/observe.js').read_text())
    ab('open', '--init-script', str(observer), BASE + '/auth/sign_in')
    ui.login()
    baseline = source_count()
    search(KNOWN, 'known')
    assert source_count() == baseline, 'local canonical lookup fetched remote'
    search(PUBLIC, 'uncached')
    cached = source_count()
    assert cached > baseline, 'uncached lookup did not use controlled source'
    search(PUBLIC, 'cached-repeat')
    assert source_count() == cached, 'cached repeat fetched remote'
    search(HIDDEN, 'hidden', False)
    assert source_count() == cached, 'known denial fetched remote'
    search(PRIVATE, 'private', False)
    save('controller-result', {'result': 'PASS', 'known_remote_fetches': 0, 'cached_repeat_remote_fetches': 0, 'hidden_remote_fetches': 0, 'source_count': source_count(), 'pagination': 'initial frontend limit=11 offset absent; singleton has no load-more control'})
    ab('close')


if __name__ == '__main__':
    main()
