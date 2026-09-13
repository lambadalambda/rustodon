#!/usr/bin/env python3
"""Offline real-TLS tests. Parent runs on NAS; no containers/DNS/global trust."""
import base64
import hashlib
import os
from pathlib import Path
import queue
import signal
import socket
import ssl
import subprocess
import sys
import tempfile
import threading
import time
import unittest

TOOLS = Path(__file__).resolve().parents[1]
HELPER = TOOLS / "browser-fixture-tls"
DOMAIN = "fixture-v4-6-5.rustodon.invalid"


def wait_file(path, process, seconds=20):
    deadline = time.monotonic() + seconds
    while not path.exists():
        if process.poll() is not None:
            raise AssertionError(f"helper exited {process.returncode} before readiness")
        if time.monotonic() >= deadline:
            raise AssertionError("bounded startup expired")
        time.sleep(.02)


def receive(sock, size):
    data = bytearray()
    while len(data) < size:
        chunk = sock.recv(min(65536, size - len(data)))
        if not chunk:
            raise AssertionError(f"truncated stream: {len(data)} / {size}")
        data.extend(chunk)
    return bytes(data)


class Transport(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.directory = self.root / "tls"
        self.directory.mkdir(mode=0o700)
        self.sibling = self.root / "unrelated"
        self.sibling.write_text("preserve")
        self.backend = socket.socket()
        self.backend.bind(("127.0.0.1", 0))
        self.backend.listen()
        self.backend.settimeout(5)
        self.addCleanup(self.backend.close)
        self.process = subprocess.Popen(
            [sys.executable, str(HELPER), "--directory", str(self.directory),
             "--domain", DOMAIN, "--backend-port", str(self.backend.getsockname()[1])],
            stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        )
        self.addCleanup(self.stop)
        wait_file(self.directory / "port", self.process)
        self.port = int((self.directory / "port").read_text())
        self.context = ssl.create_default_context(cafile=str(self.directory / "ca.pem"))

    def stop(self):
        if self.process.poll() is None:
            self.process.terminate()
        try:
            self.output = self.process.communicate(timeout=5)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.communicate(timeout=3)
            self.fail("TLS process did not exit within five seconds")

    def connect(self, context=None, hostname=DOMAIN):
        raw = socket.create_connection(("127.0.0.1", self.port), timeout=5)
        try:
            return (context or self.context).wrap_socket(raw, server_hostname=hostname)
        except BaseException:
            raw.close()
            raise

    def backend_job(self, action):
        errors = queue.Queue()

        def run():
            try:
                with self.backend.accept()[0] as sock:
                    sock.settimeout(5)
                    action(sock)
            except BaseException as error:
                errors.put(error)

        worker = threading.Thread(target=run, daemon=True)
        worker.start()

        def finish():
            worker.join(timeout=7)
            self.assertFalse(worker.is_alive(), "backend forwarding stalled")
            if not errors.empty():
                raise errors.get()

        return finish

    def test_real_ca_hostname_san_and_leaf_spki(self):
        with self.connect() as client:
            self.assertEqual(client.getpeercert()["subjectAltName"], (("DNS", DOMAIN),))
        for context, hostname in [(ssl.create_default_context(), DOMAIN),
                                  (self.context, "wrong.rustodon.invalid")]:
            with self.subTest(hostname=hostname):
                with self.assertRaises(ssl.SSLCertVerificationError):
                    self.connect(context, hostname)
        public = subprocess.run(
            ["openssl", "x509", "-in", str(self.directory / "leaf.pem"), "-pubkey", "-noout"],
            check=True, capture_output=True, timeout=5,
        ).stdout
        der = subprocess.run(["openssl", "pkey", "-pubin", "-outform", "DER"],
                             input=public, check=True, capture_output=True, timeout=5).stdout
        self.assertEqual((self.directory / "spki").read_text().strip(),
                         base64.b64encode(hashlib.sha256(der).digest()).decode())
        self.assertEqual(self.directory.stat().st_mode & 0o777, 0o700)
        keys = list(self.directory.glob("*.key"))
        self.assertTrue(keys)
        for key in keys:
            self.assertEqual(key.stat().st_mode & 0o777, 0o600)

    def test_post_patch_streams_exact_bytes_duplicate_cookies_and_large_assets(self):
        body = bytes(range(256)) * 12289  # Exceeds shared peer proxy's 2 MiB limit.
        response = (b"HTTP/1.1 200 OK\r\nSet-Cookie: a=one; Secure\r\n"
                    b"Set-Cookie: b=two; Secure\r\nContent-Length: "
                    + str(len(body)).encode() + b"\r\nConnection: close\r\n\r\n" + body)
        for method in (b"POST", b"PATCH", b"PUT"):
            with self.subTest(method=method):
                header = (method + b" /api/web/settings HTTP/1.1\r\nHost: " + DOMAIN.encode()
                          + b"\r\nExpect: 100-continue\r\nContent-Length: "
                          + str(len(body)).encode() + b"\r\n\r\n")

                def exchange(sock):
                    self.assertEqual(receive(sock, len(header)), header)
                    sock.sendall(b"HTTP/1.1 100 Continue\r\n\r\n")
                    self.assertEqual(receive(sock, len(body)), body)
                    sock.sendall(response)
                    sock.shutdown(socket.SHUT_WR)

                finish = self.backend_job(exchange)
                with self.connect() as client:
                    client.sendall(header)
                    interim = b"HTTP/1.1 100 Continue\r\n\r\n"
                    self.assertEqual(receive(client, len(interim)), interim)
                    for offset in range(0, len(body), 32768):
                        client.sendall(body[offset:offset + 32768])
                    self.assertEqual(receive(client, len(response)), response)
                    self.assertEqual(client.recv(1), b"")
                finish()
        self.stop()
        self.assertEqual(self.output, (b"", b""), "transport must not log payloads")

    def test_opaque_bidirectional_upgrade_bytes(self):
        frames = [b"\x00\xffclient-one", b"second-client\x80" * 8192]

        def exchange(sock):
            sock.sendall(b"server-first")
            for frame in frames:
                self.assertEqual(receive(sock, len(frame)), frame)
                sock.sendall(frame[::-1])

        finish = self.backend_job(exchange)
        with self.connect() as client:
            self.assertEqual(receive(client, 12), b"server-first")
            for frame in frames:
                client.sendall(frame)
                self.assertEqual(receive(client, len(frame)), frame[::-1])
            self.assertEqual(client.recv(1), b"")
        finish()

    def test_client_tls_close_ends_backend_connection_without_half_close_wait(self):
        def exchange(sock):
            self.assertEqual(receive(sock, 7), b"request")
            self.assertEqual(sock.recv(1), b"")

        finish = self.backend_job(exchange)
        with self.connect() as client:
            client.sendall(b"request")
            client.settimeout(2)
            with client.unwrap():
                pass
        finish()
        self.stop()
        self.assertEqual(self.output, (b"", b""))

    def test_term_closes_active_and_stalled_handshake_and_removes_owned_files(self):
        with self.connect() as active, socket.create_connection(("127.0.0.1", self.port)) as stalled:
            self.stop()
            self.assertEqual(self.process.returncode, 0)
            self.assertEqual(active.recv(1), b"")
            stalled.settimeout(2)
            try:
                self.assertEqual(stalled.recv(1), b"")
            except ConnectionResetError:
                pass
        with self.assertRaises(OSError):
            socket.create_connection(("127.0.0.1", self.port), timeout=1)
        self.assertFalse(self.directory.exists())
        self.assertEqual(self.sibling.read_text(), "preserve")


class FailureAndFixtureLifecycle(unittest.TestCase):
    def test_asymmetric_progress_does_not_expire_quiet_direction(self):
        import asyncio
        import runpy

        copy_stream = runpy.run_path(str(HELPER))["copy_stream"]
        copy_stream.__globals__["IDLE_SECONDS"] = .05

        class Sink:
            def __init__(self):
                self.data = bytearray()

            def write(self, data):
                self.data.extend(data)

            async def drain(self):
                pass

        async def exercise():
            quiet, active = asyncio.StreamReader(), asyncio.StreamReader()
            quiet_sink, active_sink = Sink(), Sink()
            pumps = [asyncio.create_task(copy_stream(quiet, quiet_sink)),
                     asyncio.create_task(copy_stream(active, active_sink))]
            try:
                for _ in range(8):
                    active.feed_data(b"event")
                    await asyncio.sleep(.025)
                    self.assertFalse(pumps[0].done(), "quiet direction killed progressing stream")
                quiet.feed_data(b"reply")
                quiet.feed_eof()
                active.feed_eof()
                await asyncio.wait_for(asyncio.gather(*pumps), 1)
                self.assertEqual(quiet_sink.data, b"reply")
                self.assertEqual(active_sink.data, b"event" * 8)
            finally:
                for task in pumps:
                    task.cancel()
                await asyncio.gather(*pumps, return_exceptions=True)

        asyncio.run(exercise())

    def test_invalid_domain_port_or_nonempty_directory_fails_closed(self):
        for domain, port, occupied in [("bad\nDNS:evil.invalid", "1234", False),
                                       ("*.invalid", "1234", False),
                                       (DOMAIN, "0", False), (DOMAIN, "65536", False),
                                       (DOMAIN, "1234", True)]:
            with self.subTest(domain=domain, port=port, occupied=occupied):
                with tempfile.TemporaryDirectory() as temporary:
                    directory = Path(temporary) / "tls"
                    directory.mkdir(mode=0o700)
                    if occupied:
                        (directory / "keep").write_text("unrelated")
                    result = subprocess.run(
                        [sys.executable, str(HELPER), "--directory", str(directory),
                         "--domain", domain, "--backend-port", port],
                        capture_output=True, timeout=5,
                    )
                    self.assertNotEqual(result.returncode, 0)
                    self.assertFalse((directory / "port").exists())
                    if occupied:
                        self.assertEqual((directory / "keep").read_text(), "unrelated")

    def test_openssl_timeout_is_bounded_and_cleans_directory(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            directory = root / "tls"
            directory.mkdir(mode=0o700)
            openssl = root / "openssl"
            openssl.write_text("#!/bin/sh\nexec sleep 60\n")
            openssl.chmod(0o700)
            result = subprocess.run(
                [sys.executable, str(HELPER), "--directory", str(directory),
                 "--domain", DOMAIN, "--backend-port", "1234"],
                env=dict(os.environ, PATH=f"{root}:{os.environ['PATH']}"),
                capture_output=True, timeout=8,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertFalse(directory.exists())

    def fixture_case(self, outcome):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "tools").mkdir()
            (root / "target").mkdir()
            (root / "tools" / "browser-fixture-tls").symlink_to(HELPER)
            (root / "target" / "unrelated").write_text("preserve")
            script = '''
MASTODON_FIXTURE_LIBRARY=1
. "$1"
ROOT=$2
LOCAL_DOMAIN=fixture-v4-6-5.rustodon.invalid
cutover_web_port=1234
CUTOVER_KEEP_ARTIFACTS=true
cleanup_containers() {
  [ ! -e "$tls_directory" ] || exit 91
  ! kill -0 "$tls_pid" 2>/dev/null || exit 92
  printf cleaned > "$ROOT/api-cleanup"
}
trap cutover_abort EXIT HUP INT TERM
cutover_start_browser_tls
tls_directory=$cutover_tls_dir
tls_pid=$cutover_tls_pid
printf '%s\n%s\n%s\n' "$tls_directory" "$tls_pid" "$cutover_tls_port" > "$ROOT/ready.tmp"
mv "$ROOT/ready.tmp" "$ROOT/ready"
case "$3" in
  success) cutover_stop_processes; cleanup_containers; trap - EXIT HUP INT TERM ;;
  failure) exit 7 ;;
  term) while :; do sleep 1; done ;;
esac
'''
            process = subprocess.Popen(["sh", "-c", script, "test", str(TOOLS / "mastodon-fixture"),
                                        str(root), outcome], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            try:
                wait_file(root / "ready", process, seconds=25)
                directory, pid, port = (root / "ready").read_text().splitlines()
                if outcome == "term":
                    process.send_signal(signal.SIGTERM)
                output = process.communicate(timeout=10)
                self.assertEqual(process.returncode, 7 if outcome == "failure" else 0, output)
                self.assertEqual((root / "api-cleanup").read_text(), "cleaned")
                self.assertFalse(Path(directory).exists())
                with self.assertRaises(ProcessLookupError):
                    os.kill(int(pid), 0)
                with self.assertRaises(OSError):
                    socket.create_connection(("127.0.0.1", int(port)), timeout=1)
                self.assertEqual((root / "target" / "unrelated").read_text(), "preserve")
            finally:
                if process.poll() is None:
                    process.terminate()
                    try:
                        process.communicate(timeout=10)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.communicate(timeout=3)

    def test_fixture_cleanup_on_success_failure_and_term_before_api_cleanup(self):
        for outcome in ("success", "failure", "term"):
            with self.subTest(outcome=outcome):
                self.fixture_case(outcome)

    def test_browser_https_contract_is_invocation_scoped(self):
        source = (TOOLS / "mastodon-fixture").read_text()
        block = source.split('case "${RUSTODON_BROWSER_SMOKE:-false}" in', 1)[1].split("false) ;;", 1)[0]
        self.assertIn("cutover_start_browser_tls", block)
        self.assertIn('RUSTODON_BROWSER_CA_FILE="$cutover_tls_dir/ca.pem"', block)
        self.assertIn('RUSTODON_BROWSER_SPKI="$cutover_tls_spki"', block)
        self.assertIn('"https://$LOCAL_DOMAIN:$cutover_tls_port" "$LOCAL_DOMAIN"', block)
        self.assertNotIn('"http://$LOCAL_DOMAIN:$cutover_web_port"', block)
        self.assertNotIn("export RUSTODON_BROWSER_CA_FILE", source)
        self.assertNotIn("export RUSTODON_BROWSER_SPKI", source)
        self.assertNotIn("tls-proxy.py", block)


if __name__ == "__main__":
    unittest.main()
