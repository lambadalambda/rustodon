#!/usr/bin/env python3
"""Bounded CLI driver. Writes sanitized assertions, not auth or raw HAR payloads."""
import json
from pathlib import Path
import re
import subprocess
import sys

BASE = 'https://fixture-v4-6-5.rustodon.invalid'
E = Path('/evidence')


def ab(*args, stdin=None):
    result = subprocess.run(['sh', '/harness/tools/remote-browser/browser', *args],
                            input=stdin, text=True, capture_output=True, timeout=45)
    if result.returncode:
        raise RuntimeError('browser command failed: ' + args[0] + '\n' + result.stderr)
    return result.stdout.strip()


def js(code):
    return json.loads(ab('eval', '--stdin', stdin=code))


def save(name, value):
    (E / (name + '.json')).write_text(json.dumps(value, indent=2) + '\n')


def wait(code):
    ab('wait', '--fn', code)


def login():
    snap = ab('snapshot', '-i')
    for label, value in [('Email', 'alice@fixture.invalid'), ('Password', 'fixture-password'),
                         ('Two-factor or recovery code', 'fixture-recovery-code')]:
        ref = re.search(r'textbox "' + label + r'"[^\n]*?\bref=(e\d+)\]', snap).group(1)
        ab('fill', '@' + ref, value)
    ref = re.search(r'button "Log in"[^\n]*?\bref=(e\d+)\]', snap).group(1)
    ab('click', '@' + ref)
    wait('Boolean(document.querySelector("textarea"))')
    wait('window.remoteBrowserEvents.some(e=>e.event === "open")')
    save('login', {'native_socket_open': True})


def pending(kind):
    # Stay on the same live home timeline through import and worker release.
    status = json.loads((E / (kind + '-identity.json')).read_text())['status']
    wait('window.remoteBrowserEvents.some(e=>e.event==="update" && e.id===' + json.dumps(status) + ')')
    wait('document.body.innerText.includes(' + json.dumps('Remote browser 8b2f49e ' + kind + ' final') + ')')
    events = js('window.remoteBrowserEvents')
    media = next(e['media'][0] for e in events if e.get('id') == status and e['event'] == 'update')
    assert media['url'] is None
    if kind == 'still':
        assert media['preview_url'] in (None, BASE + '/media_proxy/' + media['id'] + '/small')
    else:
        assert media['preview_url'] is None
    save(kind + '-pending-events', events)
    ab('screenshot', '/evidence/' + kind + '-pending.png')


def updated(kind):
    identity = (E / (kind + '-identity.json')).read_text()
    status = json.loads(identity)['status']
    wait('window.remoteBrowserEvents.some(e=>e.event==="status.update" && e.id===' + json.dumps(status) + ')')
    events = js('window.remoteBrowserEvents')
    save(kind + '-updated-events', events)
    media = next(e['media'][0] for e in reversed(events) if e.get('id') == status and e['event'] == 'status.update')
    assert_media(kind, media)
    save(kind + '-media', media)
    ab('screenshot', '/evidence/' + kind + '-updated.png')


def assert_media(kind, media):
    assert media['url'].startswith(BASE + '/'), media
    if kind == 'audio':
        assert media['preview_url'] is None and 'small' not in media['meta'], media
    else:
        assert media['preview_url'].startswith(BASE + '/'), media


def reload_media(kind, status, media_id, responses):
    matches = [m for r in responses if r['id'] == status
               for m in r['media'] if m['id'] == media_id]
    assert matches, 'missing actual frontend reload response'
    media = matches[-1]
    assert_media(kind, media)
    return media


def verify(kind, stage):
    media = json.loads((E / (kind + '-media.json')).read_text())
    if stage == 'reload':
        status = json.loads((E / (kind + '-identity.json')).read_text())['status']
        wait('window.remoteBrowserRest.some(r=>r.id===' + json.dumps(status) + ')')
        media = reload_media(kind, status, media['id'], js('window.remoteBrowserRest'))
        save(kind + '-reload-response', media)
    if kind == 'still':
        urls = json.dumps([media['url'], media['preview_url']])
        wait('Array.from(document.images).some(i=>' + urls + '.includes(i.src) && i.naturalWidth>0)')
        result = js('(async()=>{const i=Array.from(document.images).find(i=>' + urls + '.includes(i.src));await i.decode();const r=await fetch(i.currentSrc);const b=new Uint8Array(await r.arrayBuffer());return {src:i.currentSrc,width:i.naturalWidth,height:i.naturalHeight,mime:r.headers.get("content-type"),magic:Array.from(b.slice(0,3))}})()')
        assert result['width'] > 0 and result['mime'] == 'image/jpeg' and result['magic'] == [255,216,255]
    else:
        tag = 'video' if kind == 'video' else 'audio'
        selector = tag + '[src="' + media['url'] + '"]'
        expression = 'document.querySelector(' + json.dumps(selector) + ')'
        wait('Boolean(' + expression + ')')
        if kind == 'video':
            before = js('(async()=>{const v=' + expression + ';const i=new Image();i.src=v.poster;await i.decode();const r=await fetch(v.poster);const b=new Uint8Array(await r.arrayBuffer());return {src:v.src,poster:v.poster,paused:v.paused,time:v.currentTime,width:i.naturalWidth,mime:r.headers.get("content-type"),magic:Array.from(b.slice(0,8))}})()')
            assert before['poster'] == media['preview_url'] and before['paused'] and before['time'] == 0
            assert before['width'] > 0 and before['mime'] == 'image/png'
            assert before['magic'] == [137, 80, 78, 71, 13, 10, 26, 10]
            save(kind + '-' + stage + '-poster-before-play', before)
        else:
            fallback = js('(()=>{const a=' + expression + ';const p=a.closest(".audio-player");return {preview:' + json.dumps(media['preview_url']) + ',images:Array.from(p.querySelectorAll("svg image")).map(i=>i.getAttribute("href"))}})()')
            assert fallback['preview'] is None and fallback['images']
            status = json.loads((E / (kind + '-identity.json')).read_text())['status']
            expected = js('(async()=>{const s=await (await fetch("/api/v1/statuses/' + status + '")).json();const i=new Image();i.src=s.account.avatar_static;await i.decode();return {url:i.src,width:i.naturalWidth}})()')
            assert expected['width'] > 0 and fallback['images'] == [expected['url']]
            fallback['decoded_avatar'] = expected
            save(kind + '-' + stage + '-avatar-fallback', fallback)
        js('window.remotePlaybackBefore=(()=>{const v=' + expression + ';return {time:v.currentTime,frames:v.getVideoPlaybackQuality?.().totalVideoFrames??null}})()')
        if kind == 'video':
            ab('click', selector)
        else:
            # Native user gesture, not an autoplay workaround or fake player.
            ab('click', '.audio-player:has(' + selector + ') button[aria-label="Play"]')
        wait('(()=>{const v=' + expression + ';return v.currentTime>window.remotePlaybackBefore.time && !v.error})()')
        result = js('(()=>{const v=' + expression + ';return {src:v.currentSrc,before:window.remotePlaybackBefore,time:v.currentTime,frames:v.getVideoPlaybackQuality?.().totalVideoFrames??null,error:v.error?.code??null}})()')
        assert result['src'] == media['url'] and result['time'] > result['before']['time'] and result['error'] is None
        if kind == 'video':
            assert result['frames'] > result['before']['frames']
    save(kind + '-' + stage + '-native', result)
    ab('screenshot', '/evidence/' + kind + '-' + stage + '.png')


def sanitize_requests(requests):
    from urllib.parse import urlsplit, urlunsplit
    records = []
    for request in requests:
        u = urlsplit(request['url'])
        records.append({'url': urlunsplit((u.scheme, u.netloc, u.path, '', '')),
                        'method': request.get('method'), 'type': request.get('resourceType'),
                        'status': request.get('status'), 'mime': request.get('mimeType')})
    return records


def assert_network(records, expected):
    from urllib.parse import urlsplit
    assert records, 'empty stage request recorder is not proof'
    assert not any(urlsplit(r['url']).hostname == 'remote.fixture.invalid' for r in records)
    for kind, media in expected:
        url = media['preview_url'] if kind == 'still' else media['url']
        assert any(r['url'] == url and r['status'] in (200, 206) for r in records), (kind, url)


def network(stage, *kinds):
    raw = json.loads(ab('network', 'requests', '--json'))
    records = sanitize_requests(raw['data']['requests'])
    save(stage + '-network', records)
    expected = [(kind, json.loads((E / (kind + ('-reload-response.json' if stage.endswith('-reload') else '-media.json'))).read_text())) for kind in kinds]
    assert_network(records, expected)
    hotlinks = js('Array.from(document.querySelectorAll("video,audio,img,source,svg image")).flatMap(e=>[e.src,e.currentSrc,e.poster,e.getAttribute("href")]).filter(x=>typeof x==="string" && x.includes("remote.fixture.invalid/media/"))')
    assert not hotlinks, hotlinks
    save(stage + '-no-hotlinks', {'requests': len(records), 'origin_requests': 0, 'dom_hotlinks': hotlinks})
    ledger = E / 'aggregate-network.json'
    aggregate = json.loads(ledger.read_text()) if ledger.exists() else []
    aggregate.extend(dict(r, stage=stage) for r in records)
    assert_network(aggregate, [])
    save('aggregate-network', aggregate)
    # Clear only after the stage and aggregate have both been saved and checked.
    ab('network', 'requests', '--clear')


if __name__ == '__main__':
    {'login': login, 'pending': pending, 'updated': updated,
     'verify': verify, 'network': network}[sys.argv[1]](*sys.argv[2:])
