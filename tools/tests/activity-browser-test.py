#!/usr/bin/env python3
import importlib.util
from pathlib import Path
import tempfile
import unittest
ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('prepare', ROOT/'tools/activity-browser/prepare.py')
p = importlib.util.module_from_spec(spec)
spec.loader.exec_module(p)

class Acceptance(unittest.TestCase):
    def test_exact_source_and_bounds(self):
        with tempfile.TemporaryDirectory() as d:
            p.prepare(ROOT, Path(d))
            run = (Path(d)/'run.sh').read_text()
            setup = (Path(d)/'setup.sh').read_text()
            self.assertIn(p.REV, run)
            self.assertNotIn('--features test-support', run)
            self.assertIn('--cpus=4 --memory=6g', run)
            self.assertNotIn('run pull ', run)
            self.assertNotIn('run source ', run)
            self.assertNotIn('RUSTODON_TEST_PEER', setup)
            self.assertIn('for mode in web; do', setup)
            self.assertIn('activity-browser/stages.sh', run)
            self.assertIn('ui.ab = ab', (ROOT/'tools/activity-browser/ui.py').read_text())
    def test_drift_rejected(self):
        with self.assertRaises(ValueError):
            p.once('x x', 'x', 'y')

if __name__ == '__main__': unittest.main()
