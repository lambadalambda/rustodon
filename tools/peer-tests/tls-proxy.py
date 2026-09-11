#!/usr/bin/env python3
"""Task-local TLS terminator; fixed backend, no routing to arbitrary destinations."""
import http.client
import http.server
import json
import pathlib
import ssl
import sys

PUBLIC = 'https://www.w3.org/ns/activitystreams#Public'


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
    audiences = [fields.get(key) for fields in (activity, obj_fields) for key in ('to', 'cc')]
    recipients = [item for value in audiences
                  for item in (value if isinstance(value, list) else [value])
                  if isinstance(item, str)]
    return dict(method=method, path=path, host=host, status=status, signed=signed,
                activity=activity.get('type'), actor=activity.get('actor'),
                object=obj_fields.get('id') if isinstance(obj, dict) else obj,
                public=PUBLIC in recipients, recipients=recipients)


class Proxy(http.server.BaseHTTPRequestHandler):
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
            conn.request(self.command, self.path, request_body, headers)
            response = conn.getresponse()
            event = audit_event(self.command, self.path, name, response.status,
                                'Signature' in self.headers, request_body)
            with pathlib.Path(root, f'{name}.jsonl').open('a') as audit:
                audit.write(json.dumps(event) + '\n')
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
