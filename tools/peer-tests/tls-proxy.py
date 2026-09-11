#!/usr/bin/env python3
"""Task-local TLS terminator; fixed backend, no routing to arbitrary destinations."""
import http.client
import http.server
import json
import pathlib
import ssl
import sys

PUBLIC = 'https://www.w3.org/ns/activitystreams#Public'


def recipients_for(fields):
    return [item for key in ('to', 'cc')
            for value in [fields.get(key)]
            for item in (value if isinstance(value, list) else [value])
            if isinstance(item, str)]


def audit_event(method, path, host, status, signed, body):
    """Record only transport/ActivityPub identities, never credentials or content."""
    try:
        activity = json.loads(body)
    except (ValueError, UnicodeDecodeError):
        activity = {}
    if not isinstance(activity, dict):
        activity = {}
    obj = activity.get('object')
    obj_fields = obj if isinstance(obj, dict) else {}
    outer_recipients = recipients_for(activity)
    recipients = outer_recipients + recipients_for(obj_fields)
    return dict(method=method, path=path, host=host, status=status, signed=signed,
                activity=activity.get('type'), activity_id=activity.get('id'),
                actor=activity.get('actor'),
                object=obj_fields.get('id') if isinstance(obj, dict) else obj,
                public=PUBLIC in recipients, recipients=recipients,
                outer_public=PUBLIC in outer_recipients, outer_recipients=outer_recipients)


class Proxy(http.server.BaseHTTPRequestHandler):
    def audit_request(self, body, status):
        event = audit_event(self.command, self.path, name, status,
                            'Signature' in self.headers, body)
        with pathlib.Path(root, f'{name}.jsonl').open('a') as audit:
            audit.write(json.dumps(event) + '\n')

    def forward(self):
        self.connection.settimeout(15)
        if self.headers.get('Host') != name or self.headers.get('Transfer-Encoding'):
            self.send_error(400)
            return
        size = int(self.headers.get('Content-Length', '0'))
        if not 0 <= size <= 2 * 1024 * 1024:
            self.send_error(413)
            return
        headers = {k: v for k, v in self.headers.items()
                   if k.lower() not in ('connection', 'x-forwarded-proto', 'x-forwarded-for')}
        headers['X-Forwarded-Proto'] = 'https'
        headers['Connection'] = 'close'
        conn = http.client.HTTPConnection('127.0.0.1', backend, timeout=15)
        try:
            request_body = self.rfile.read(size)
            # Preserve attempts even if forwarding fails or response headers never arrive.
            self.audit_request(request_body, None)
            conn.request(self.command, self.path, request_body, headers)
            response = conn.getresponse()
            self.audit_request(request_body, response.status)
            body = response.read(2 * 1024 * 1024 + 1)
            if len(body) > 2 * 1024 * 1024:
                self.send_error(502)
                return
            self.send_response(response.status)
            for key, value in response.getheaders():
                if key.lower() not in ('connection', 'transfer-encoding', 'content-length'):
                    self.send_header(key, value)
            self.send_header('Content-Length', str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        finally:
            conn.close()

    do_GET = forward
    do_POST = forward


if __name__ == '__main__':
    root, name, backend = sys.argv[1:]
    backend = int(backend)
    server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Proxy)
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain(f'{root}/server.crt', f'{root}/server.key')
    server.socket = context.wrap_socket(server.socket, server_side=True)
    pathlib.Path(root, f'{name}.port').write_text(str(server.server_port))
    server.serve_forever()
