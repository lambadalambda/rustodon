#!/usr/bin/env python3
"""Real pinned frontend clicks; no fetch, Redux writes or synthetic responses."""
import importlib.util
import json
from pathlib import Path
import subprocess
import sys

spec = importlib.util.spec_from_file_location('base_ui', '/harness/tools/remote-browser/ui.py')
ui = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ui)
BASE = ui.BASE
SID = '119000000000000001'


def ab(*args, stdin=None):
    result = subprocess.run(['sh', '/harness/tools/hashtag-runtime/browser', *args], input=stdin, text=True, capture_output=True, timeout=45)
    if result.returncode:
        raise RuntimeError('browser command failed: ' + args[0] + '\n' + result.stderr)
    return result.stdout.strip()


ui.ab = ab
js, wait, save = ui.js, ui.wait, ui.save


def record(label):
    save(label, {'calls': js('window.hashtagCalls'), 'text': js('document.body.innerText')})
    ab('screenshot', '/evidence/' + label + '.png')


def button(name):
    ab('find', 'role', 'button', 'click', '--name', name, '--exact')


def tag_ready(following, featuring):
    wait('Boolean(document.querySelector(".hashtag-header"))')
    wait('window.hashtagCalls.some(r=>r.method==="GET" && r.path.toLowerCase()==="/api/v1/tags/fixturetag" && r.code===200)')
    tag = js('window.hashtagCalls.filter(r=>r.method==="GET" && r.path.toLowerCase()==="/api/v1/tags/fixturetag").at(-1).tags')
    assert tag['following'] is following and tag['featuring'] is featuring, tag
    assert len(tag['history']) == 7 and sum(int(x['uses']) for x in tag['history']) >= 2, tag
    assert 'posts' in js('document.querySelector(".hashtag-header").innerText')


def mutation(action):
    wait('window.hashtagCalls.some(r=>r.method==="POST" && r.path.toLowerCase()===' + json.dumps('/api/v1/tags/fixturetag/' + action) + ' && r.code===200)')


def feature(action, label):
    ab('click', '.hashtag-header .icon-button')
    ab('find', 'text', label, 'click', '--exact')
    mutation(action)
    record('header-' + action)


def home(expected, label):
    ab('open', BASE + '/home')
    wait('window.hashtagCalls.some(r=>r.path==="/api/v1/timelines/home" && r.code===200)')
    ids = js('window.hashtagCalls.filter(r=>r.path==="/api/v1/timelines/home").flatMap(r=>r.ids)')
    assert (SID in ids) is expected, ids
    if expected:
        wait('document.body.innerText.includes("Hashtag acceptance home inclusion")')
    else:
        assert 'Hashtag acceptance home inclusion' not in js('document.body.innerText')
    record(label)


def public_profile(expected, label):
    ab('open', BASE + '/@alice')
    wait('window.hashtagCalls.some(r=>r.path.endsWith("/featured_tags") && r.code===200)')
    tags = js('window.hashtagCalls.filter(r=>r.path.endsWith("/featured_tags") && r.code===200).at(-1).tags')
    assert any(t['name'].lower() == 'fixturetag' for t in tags) is expected, tags
    if expected:
        wait('Array.from(document.querySelectorAll("button")).some(b=>b.textContent==="#FixtureTag")')
    record(label)


def main():
    observer = Path('/run-fixture/hashtag-observe.js')
    observer.write_text(Path('/harness/tools/remote-browser/observe.js').read_text() + '\n' + Path('/harness/tools/hashtag-controls-browser/observe.js').read_text())
    ab('open', '--init-script', str(observer), BASE + '/auth/sign_in')
    ui.login()
    home(False, 'home-before')
    ab('open', BASE + '/tags/FixtureTag')
    tag_ready(False, False)
    record('header-history')
    button('Follow hashtag')
    mutation('follow')
    wait('document.querySelector(".hashtag-header").innerText.includes("Unfollow hashtag")')
    record('header-follow')
    ab('reload')
    tag_ready(True, False)
    record('follow-reload')
    home(True, 'home-followed')
    ab('open', BASE + '/tags/FixtureTag')
    tag_ready(True, False)
    button('Unfollow hashtag')
    mutation('unfollow')
    record('header-unfollow')
    ab('reload')
    tag_ready(False, False)
    record('unfollow-reload')
    home(False, 'home-unfollowed')
    ab('open', BASE + '/tags/FixtureTag')
    tag_ready(False, False)
    feature('feature', 'Feature on profile')
    ab('reload')
    tag_ready(False, True)
    record('feature-reload')
    public_profile(True, 'public-featured')
    ab('open', BASE + '/tags/FixtureTag')
    tag_ready(False, True)
    feature('unfeature', "Don't feature on profile")
    ab('reload')
    tag_ready(False, False)
    record('unfeature-reload')
    public_profile(False, 'public-unfeatured')
    ab('open', BASE + '/profile/featured_tags')
    wait('window.hashtagCalls.some(r=>r.path==="/api/v1/profile" && r.code===200)')
    wait('document.body.innerText.includes("Suggestions:")')
    # Existing genuine suggestion button invokes collection POST, not header feature.
    button('#FixtureTag')
    wait('window.hashtagCalls.some(r=>r.method==="POST" && r.path==="/api/v1/featured_tags" && r.code===200)')
    record('profile-add')
    ab('reload')
    wait(r'Boolean(document.querySelector("button[aria-label=\"Delete FixtureTag\"]"))')
    record('profile-add-reload')
    public_profile(True, 'public-profile-added')
    ab('open', BASE + '/profile/featured_tags')
    wait(r'Boolean(document.querySelector("button[aria-label=\"Delete FixtureTag\"]"))')
    button('Delete FixtureTag')
    wait('window.hashtagCalls.some(r=>r.method==="DELETE" && r.path.startsWith("/api/v1/featured_tags/") && r.code===200)')
    record('profile-remove')
    ab('reload')
    wait('window.hashtagCalls.some(r=>r.method==="GET" && r.path==="/api/v1/profile" && r.code===200)')
    assert not js(r'Boolean(document.querySelector("button[aria-label=\"Delete FixtureTag\"]"))')
    record('profile-remove-reload')
    public_profile(False, 'public-profile-removed')
    save('controller-result', {'result':'PASS', 'mutations':'actual UI clicks', 'history':'current DB counts, not Redis retention parity'})
    ab('close')


def editor(count, label):
    # Pinned UI dispatches fetchServer after 3000 ms; profile loaded != editor ready.
    wait('window.hashtagCalls.some(r=>r.path==="/api/v2/instance" && r.code===200 && r.max_featured_tags===10)')
    wait('window.hashtagCalls.some(r=>r.path==="/api/v1/profile" && r.code===200)')
    wait('Array.from(document.querySelectorAll("button")).filter(b=>(b.getAttribute("aria-label") || "").startsWith("Delete ")).length===' + str(count))
    if count < 10:
        wait('Boolean(document.querySelector("input[type=search]"))')
        assert 'You have reached the maximum number' not in js('document.body.innerText')
    else:
        wait('document.body.innerText.includes("You have reached the maximum number of featured hashtags.")')
        assert not js('Boolean(document.querySelector("input[type=search]"))')
    record(label)


def typed_main():
    observer = Path('/run-fixture/hashtag-observe.js')
    observer.write_text(Path('/harness/tools/remote-browser/observe.js').read_text() + '\n' + Path('/harness/tools/hashtag-controls-browser/observe.js').read_text())
    ab('open', '--init-script', str(observer), BASE + '/auth/sign_in')
    ui.login()
    ab('open', BASE + '/profile/featured_tags')
    editor(0, 'typed-empty')
    ab('reload')
    editor(0, 'typed-empty-reload')
    names = ['TypedLimit' + str(i) for i in range(10)]
    for i, name in enumerate(names):
        previous = js('window.hashtagCalls.filter(r=>r.method==="POST" && r.path==="/api/v1/featured_tags" && r.code===200).length')
        ab('fill', 'input[type=search]', name)
        ab('find', 'text', 'Add #' + name, 'click', '--exact')
        wait('window.hashtagCalls.filter(r=>r.method==="POST" && r.path==="/api/v1/featured_tags" && r.code===200).length===' + str(previous + 1))
        editor(i + 1, 'typed-add-' + str(i + 1))
        if i == 0:
            ab('reload')
            editor(1, 'typed-one-reload')
            typed_public(names[:1], 'typed-public-one')
            ab('open', BASE + '/profile/featured_tags')
            editor(1, 'typed-one-return')
    ab('reload')
    editor(10, 'typed-limit-reload')
    typed_public(names, 'typed-public-ten')
    ab('open', BASE + '/profile/featured_tags')
    editor(10, 'typed-limit-return')
    for i, name in enumerate(names):
        previous = js('window.hashtagCalls.filter(r=>r.method==="DELETE" && r.path.startsWith("/api/v1/featured_tags/") && r.code===200).length')
        button('Delete ' + name)
        wait('window.hashtagCalls.filter(r=>r.method==="DELETE" && r.path.startsWith("/api/v1/featured_tags/") && r.code===200).length===' + str(previous + 1))
        editor(9 - i, 'typed-delete-' + str(i + 1))
        if i == 0:
            ab('reload')
            editor(9, 'typed-below-limit-reload')
    ab('reload')
    editor(0, 'typed-removed-reload')
    typed_public([], 'typed-public-removed')
    save('controller-result', {'result': 'PASS', 'mutations': '10 typed Add clicks and 10 Delete clicks', 'limit': 10})
    ab('close')


def typed_public(names, label):
    ab('open', BASE + '/@alice')
    wait('window.hashtagCalls.some(r=>r.path.endsWith("/featured_tags") && r.code===200)')
    tags = js('window.hashtagCalls.filter(r=>r.path.endsWith("/featured_tags") && r.code===200).at(-1).tags')
    assert sorted(t['name'] for t in tags) == sorted(names), tags
    for name in names:
        wait('Array.from(document.querySelectorAll("button")).some(b=>b.textContent===' + json.dumps('#' + name) + ')')
    if not names:
        assert 'TypedLimit' not in js('document.body.innerText')
    record(label)


if __name__ == '__main__':
    try:
        typed_main() if sys.argv[1:] == ["typed"] else main()
    except Exception:
        # Sanitized DOM/call evidence, no auth inputs or raw network payloads.
        save('failure', {'calls':js('window.hashtagCalls'), 'snapshot':ab('snapshot', '-i')})
        raise
