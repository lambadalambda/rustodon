#!/usr/bin/env python3
"""Bounded readiness of the actual canonical TLS chain, before browser launch."""
import socket
import ssl
import time
from pathlib import Path


def main():
    root = Path('/run-fixture')
    checks = [(443, 'fixture-v4-6-5.rustodon.invalid', root / 'tls/ca.pem', '/auth/sign_in', b'200'),
              (19443, 'remote.fixture.invalid', root / 'remote.pem', '/not-media', b'404')]
    for port, host, ca, path, expected in checks:
        deadline = time.monotonic() + 30
        context = ssl.create_default_context(cafile=str(ca))
        while True:
            try:
                with socket.create_connection(('127.0.0.1', port), timeout=2) as sock:
                    with context.wrap_socket(sock, server_hostname=host) as tls:
                        tls.sendall(f'GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n'.encode())
                        response = tls.makefile('rb').readline(512)
                        assert response.split()[1] == expected
                print(f'ready: {host}:{port}, verified CA and HTTP {expected.decode()}', flush=True)
                break
            except (OSError, AssertionError, IndexError):
                if time.monotonic() >= deadline:
                    raise RuntimeError(f'readiness failed: {host}:{port}') from None
                time.sleep(.2)


if __name__ == '__main__':
    main()
