#!/usr/bin/env python3
"""Opt-in task-local TLS media source and signed Note delivery. Not an app server."""
import base64
import email.utils
import hashlib
import http.client
import http.server
import json
from pathlib import Path
import ssl
import subprocess
import sys
import time

ORIGIN = 'https://remote.fixture.invalid'
ACTOR = ORIGIN + '/users/bob'
PUBLIC = 'https://www.w3.org/ns/activitystreams#Public'
LOCAL = 'fixture-v4-6-5.rustodon.invalid'
MEDIA = {'video': ('capability.mp4', 'video/mp4'),
         'audio': ('boop.ogg', 'audio/ogg'),
         'still': ('600x400.avif', 'image/avif')}
ROOT = Path('/run-fixture')


def activity(kind):
    _, mime = MEDIA[kind]
    identity = ORIGIN + '/notes/remote-browser-8b2f49e-r2-final-' + kind
    return {'@context': 'https://www.w3.org/ns/activitystreams', 'type': 'Create',
            'id': identity + '/activity', 'actor': ACTOR, 'to': [PUBLIC],
            'cc': [ACTOR + '/followers'],
            'object': {'type': 'Note', 'id': identity, 'attributedTo': ACTOR,
                       'published': email.utils.formatdate(usegmt=True),
                       'content': '<p>Remote browser 8b2f49e ' + kind + ' final</p>',
                       'contentMap': {'en': '<p>Remote browser 8b2f49e ' + kind + ' final</p>'},
                       'to': [PUBLIC], 'cc': [ACTOR + '/followers'],
                       'attachment': [{'type': 'Document', 'mediaType': mime,
                                       'url': ORIGIN + '/media/' + kind}]}}


def audit(path, signed, phase):
    return {'path': path, 'signed': bool(signed), 'phase': phase}


class Source(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def do_GET(self):
        kind = self.path.removeprefix('/media/')
        if self.headers.get('Host') != 'remote.fixture.invalid' or kind not in MEDIA:
            self.send_error(404)
            return
        def record(phase):
            with (ROOT / 'source.jsonl').open('a') as out:
                out.write(json.dumps(audit(self.path, 'Signature' in self.headers, phase)) + '\n')
        record('held')
        deadline = time.monotonic() + 240
        while not (ROOT / ('release-' + kind)).exists():
            if time.monotonic() > deadline:
                self.send_error(503)
                record('timeout')
                return
            time.sleep(.1)
        filename, mime = MEDIA[kind]
        body = (Path('/workspace/tests/fixtures/media') / filename).read_bytes()
        self.send_response(200)
        self.send_header('Content-Type', mime)
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)
        record('released')


def serve():
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.minimum_version = ssl.TLSVersion.TLSv1_2
    context.load_cert_chain(ROOT / 'remote-leaf.pem', ROOT / 'remote.key')
    server = http.server.ThreadingHTTPServer(('127.0.0.1', 19443), Source)
    server.socket = context.wrap_socket(server.socket, server_side=True)
    server.serve_forever()


def deliver(kind):
    from datetime import datetime, timezone
    document = activity(kind)
    document['object']['published'] = datetime.now(timezone.utc).isoformat()
    body = json.dumps(document, separators=(',', ':')).encode()
    date = email.utils.formatdate(usegmt=True)
    digest = 'SHA-256=' + base64.b64encode(hashlib.sha256(body).digest()).decode()
    signing = f'(request-target): post /inbox\nhost: {LOCAL}\ndate: {date}\ndigest: {digest}'
    signature = subprocess.run(['openssl', 'dgst', '-sha256', '-sign', str(ROOT / 'actor.key')],
                               input=signing.encode(), capture_output=True, check=True, timeout=5).stdout
    headers = {'Host': LOCAL, 'Date': date, 'Digest': digest,
               'Content-Type': 'application/activity+json',
               'Signature': f'keyId="{ACTOR}#main-key",algorithm="rsa-sha256",headers="(request-target) host date digest",signature="{base64.b64encode(signature).decode()}"'}
    conn = http.client.HTTPConnection('127.0.0.1', 18374, timeout=20)
    try:
        conn.request('POST', '/inbox', body, headers)
        response = conn.getresponse()
        response.read()
        print(json.dumps({'kind': kind, 'status': response.status, 'activity': document['id']}))
        if response.status != 202:
            raise RuntimeError('signed inbox delivery rejected')
    finally:
        conn.close()


if __name__ == '__main__':
    if sys.argv[1:] == ['serve']:
        serve()
    elif len(sys.argv) == 3 and sys.argv[1] == 'deliver':
        deliver(sys.argv[2])
    else:
        raise SystemExit('usage: fixture.py serve | deliver video|audio|still')
