#!/usr/bin/env python3
"""Exercise process cleanup without Podman, databases, or privileged operations."""
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import time
import unittest

RUNNER = Path(__file__).resolve().parents[1] / "nas-fixture-session"
API = '''import os, pathlib, socket
pathlib.Path(os.environ['PROBE_ROOT'], 'api.pid').write_text(str(os.getpid()))
s = socket.socket(socket.AF_UNIX)
s.bind(os.environ['RUSTODON_NAS_SOCKET'])
s.listen()
while True:
 c, _ = s.accept(); c.close()
'''
COMMAND = '''import os, pathlib, signal, socket, subprocess, sys, time
root = pathlib.Path(os.environ['PROBE_ROOT'])
root.joinpath('command.pid').write_text(str(os.getpid()))
if sys.argv[1] != 'wait': sys.exit(int(sys.argv[1]))
child = subprocess.Popen(['sleep', '60'])
root.joinpath('child.pid').write_text(str(child.pid))
def stop(sig, frame):
 s = socket.socket(socket.AF_UNIX); s.connect(os.environ['RUSTODON_NAS_SOCKET']); s.close()
 root.joinpath('cleanup-used-api').write_text('yes')
 child.wait(timeout=3)
 sys.exit(0)
signal.signal(signal.SIGTERM, stop)
root.joinpath('ready').touch()
while True: time.sleep(.1)
'''


class Lifecycle(unittest.TestCase):
    def run_case(self, outcome):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'source').mkdir()
            (root / 'api.py').write_text(API)
            (root / 'command.py').write_text(COMMAND)
            env = dict(os.environ, PROBE_ROOT=str(root))
            script = '''RUSTODON_NAS_SESSION_LIBRARY=1 source "$1"
ROOT="$PROBE_ROOT/source"
nas_session_api() { exec python3 "$PROBE_ROOT/api.py"; }
nas_session_run python3 "$PROBE_ROOT/command.py" "$2"
'''
            process = subprocess.Popen(['bash', '-c', script, 'test', str(RUNNER), outcome],
                                       env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            try:
                if outcome == 'wait':
                    deadline = time.monotonic() + 8
                    while not (root / 'ready').exists():
                        if process.poll() is not None or time.monotonic() > deadline:
                            self.fail('command failed to become ready')
                        time.sleep(.02)
                    process.send_signal(signal.SIGTERM)
                stdout, stderr = process.communicate(timeout=20)
                expected = 143 if outcome == 'wait' else int(outcome)
                self.assertEqual(process.returncode, expected, (stdout, stderr))
                if outcome == 'wait':
                    self.assertEqual((root / 'cleanup-used-api').read_text(), 'yes')
                self.assertFalse(list((root / 'ops').glob('*/podman.sock')))
                for file in root.glob('*.pid'):
                    with self.assertRaises(ProcessLookupError, msg=f'leaked {file.name}'):
                        os.kill(int(file.read_text()), 0)
            finally:
                if process.poll() is None:
                    process.kill()
                    process.communicate(timeout=5)

    def test_success_and_failure_exit_status(self):
        for status in ['0', '7']:
            with self.subTest(status=status):
                self.run_case(status)

    def test_term_allows_fixture_cleanup_before_api_shutdown(self):
        self.run_case('wait')


if __name__ == '__main__':
    unittest.main()
