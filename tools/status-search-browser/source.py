#!/usr/bin/env python3
"""Controlled canonical TLS Note source; verifies signed GET, retains no headers."""
import base64
import importlib.util
import json
from pathlib import Path
import re
import subprocess
import tempfile

# Reuse TLS server/lifecycle from the media fixture, not media routes or delivery.
spec = importlib.util.spec_from_file_location('media_fixture', Path(__file__).resolve().parents[1] / 'remote-browser/fixture.py')
media = importlib.util.module_from_spec(spec)
spec.loader.exec_module(media)
ROOT = media.ROOT
ORIGIN = media.ORIGIN
ACTOR = media.ACTOR
PREFIX = '/notes/status-search-c3497f5-'


def verified(handler):
    try:
        fields = dict(re.findall(r'(\w+)="([^"]+)"', handler.headers.get('Signature', '')))
        assert fields['keyId'] == 'https://fixture-v4-6-5.rustodon.invalid/actor#main-key'
        names = fields['headers'].split()
        assert '(request-target)' in names and 'host' in names and 'date' in names
        message = '\n'.join(n + ': ' + ('get ' + handler.path if n == '(request-target)' else handler.headers[n]) for n in names)
        with tempfile.NamedTemporaryFile() as signature:
            signature.write(base64.b64decode(fields['signature'], validate=True))
            signature.flush()
            result = subprocess.run(['openssl', 'dgst', '-sha256', '-verify', str(ROOT / 'searcher.pub'), '-signature', signature.name], input=message.encode(), capture_output=True, timeout=5)
            return result.returncode == 0
    except (KeyError, AssertionError, ValueError, TypeError):
        return False


class Source(media.Source):
    def do_GET(self):
        # The one unsigned readiness probe is not an application fetch. Count
        # every other source request, including unsupported paths, without logs
        # containing arbitrary query strings or headers.
        if self.path == '/not-media':
            self.send_error(404)
            return
        allowed = self.headers.get('Host') == 'remote.fixture.invalid' and self.path in (PREFIX + 'public', PREFIX + 'private', '/users/bob')
        valid = verified(self)
        with (ROOT / 'source.jsonl').open('a') as out:
            out.write(json.dumps({'path': self.path if allowed else '<unexpected>', 'signature_valid': valid}) + '\n')
        if not allowed:
            self.send_error(404)
            return
        if not valid:
            self.send_error(401)
            return
        if self.path == '/users/bob':
            document = {'@context': 'https://www.w3.org/ns/activitystreams', 'type': 'Person', 'id': ACTOR, 'preferredUsername': 'bob', 'inbox': ACTOR + '/inbox', 'outbox': ACTOR + '/outbox', 'followers': ACTOR + '/followers', 'publicKey': {'id': ACTOR + '#main-key', 'owner': ACTOR, 'publicKeyPem': (ROOT / 'actor.pub').read_text()}}
        else:
            public = self.path.endswith('-public')
            document = {'@context': 'https://www.w3.org/ns/activitystreams', 'type': 'Note', 'id': ORIGIN + self.path, 'url': ORIGIN + self.path, 'attributedTo': ACTOR, 'published': '2026-07-01T12:00:00Z', 'content': '<p>Status search c3497f5 ' + ('public accepted' if public else 'PRIVATE MUST NOT DISPLAY') + '</p>', 'to': [media.PUBLIC] if public else [ORIGIN + '/users/carol'], 'cc': []}
        body = json.dumps(document).encode()
        self.send_response(200)
        self.send_header('Content-Type', 'application/activity+json')
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)


if __name__ == '__main__':
    media.Source = Source
    media.serve()
