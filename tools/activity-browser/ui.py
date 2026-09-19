#!/usr/bin/env python3
"""Actual public sidebar, native login, reload and same-origin HTTP observations."""
import importlib.util
import json
from pathlib import Path
import re
import subprocess
import sys

spec = importlib.util.spec_from_file_location('base_ui', '/harness/tools/remote-browser/ui.py')
ui = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ui)


def ab(*args, stdin=None):
    result = subprocess.run(['sh', '/harness/tools/activity-runtime/browser', *args],
                            input=stdin, text=True, capture_output=True, timeout=45)
    if result.returncode:
        raise RuntimeError('browser command failed: ' + args[0] + '\n' + result.stderr)
    return result.stdout.strip()


ui.ab = ab
js, wait, save = ui.js, ui.wait, ui.save


def observe(label, expected, raw, sidebar=True):
    wait('Boolean(document.querySelector("#initial-state"))')
    if sidebar:
        wait('window.activityCalls.some(r=>r.status===200 && r.active_month===' + str(expected) + ')')
        wait('Boolean(document.querySelector(".server-banner__number")) && document.querySelector(".server-banner__number").innerText.trim()===' + json.dumps(str(expected)))
    values = js('''(async()=>{
      const v = await fetch('/api/v2/instance'); const n = await fetch('/nodeinfo/2.0');
      const v2=await v.json(), node=await n.json();
      const initial=JSON.parse(document.querySelector('#initial-state').textContent);
      return {v2_status:v.status,node_status:n.status,month:v2.usage.users.active_month,
        node:node.usage.users,initial_active_month:initial.instance.usage.users.active_month, calls:window.activityCalls,
        sidebar:document.querySelector('.server-banner__number')?.innerText??null};
    })()''')
    # Retain only count-bearing metadata, never access tokens or session data.
    assert values['v2_status'] == values['node_status'] == 200, values
    assert values['month'] == values['initial_active_month'] == expected, values
    assert values['node']['activeMonth'] == values['node']['activeHalfyear'] == raw, values
    save(label, values)
    ab('screenshot', '/evidence/' + label + '.png')


def main():
    stage = sys.argv[1]
    if stage == 'login':
        ab('open', ui.BASE + '/auth/sign_in')
        snap = ab('snapshot', '-i')
        for label, value in [('Email', 'alice@fixture.invalid'), ('Password', 'fixture-password'),
                             ('Two-factor or recovery code', 'fixture-recovery-code')]:
            ref = re.search(r'textbox "' + label + r'"[^\n]*?\bref=(e\d+)\]', snap).group(1)
            ab('fill', '@' + ref, value)
        ref = re.search(r'button "Log in"[^\n]*?\bref=(e\d+)\]', snap).group(1)
        ab('click', '@' + ref)
        wait('Boolean(document.querySelector("textarea"))')
        observe('today-after-real-login', 0, 0, sidebar=False)
        ab('close')  # Next stage is a fresh public, unauthenticated browser.
        return
    expected = 1 if stage == 'historical' else 0
    raw = 1 if stage in ('historical', 'limited') else 0
    ab('open', '--init-script', '/harness/tools/activity-browser/observe.js', ui.BASE + '/public/local')
    observe(stage, expected, raw, sidebar=True)
    ab('reload')
    observe(stage + '-reload', expected, raw, sidebar=True)
    (Path('/evidence')/(stage + '-snapshot.txt')).write_text(ab('snapshot'))


if __name__ == '__main__': main()
