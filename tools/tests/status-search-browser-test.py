#!/usr/bin/env python3
"""Offline guards for the fixed status-search adaptation (no containers)."""
import base64
import importlib.util
import os
from pathlib import Path
import subprocess
import tempfile
from types import SimpleNamespace
import unittest

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('prepare', ROOT / 'tools/status-search-browser/prepare.py')
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)


class Adaptation(unittest.TestCase):
    def test_fixed_controller(self):
        with tempfile.TemporaryDirectory() as d:
            out = Path(d)
            m.prepare(ROOT, out)
            run = (out / 'run.sh').read_text()
            self.assertIn(m.REV, run)
            self.assertNotIn('8b2f49e', run)
            self.assertNotIn('accept.sh', run)
            self.assertNotIn('run pull ', run)
            self.assertIn('status-search-browser/ui.py', run)
            self.assertIn('timeout -k 10 600', run)
            self.assertIn('trap ', run)
            self.assertIn('--memory=6g', run)
            setup = (out / 'setup.sh').read_text()
            self.assertIn('for mode in web;', setup)
            self.assertIn('browser_writer', setup)
            self.assertIn('migrate-operational-schema', setup)
            self.assertIn('searcher.pub', setup)

    def test_instance_signer_not_viewer(self):
        source = (ROOT / 'tools/status-search-browser/source.py').read_text()
        self.assertIn('/actor#main-key', source)
        with tempfile.TemporaryDirectory() as d:
            m.prepare(ROOT, Path(d))
            self.assertIn('SELECT public_key FROM accounts WHERE id=-99', (Path(d) / 'setup.sh').read_text())

    def test_signed_source_verification(self):
        spec = importlib.util.spec_from_file_location('source', ROOT / 'tools/status-search-browser/source.py')
        source = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(source)
        key = ROOT / 'tests/fixtures/http-signature-private.pem'
        with tempfile.TemporaryDirectory() as d:
            source.ROOT = Path(d)
            pub = subprocess.run(['openssl', 'pkey', '-in', str(key), '-pubout'], capture_output=True, check=True).stdout
            (source.ROOT / 'searcher.pub').write_bytes(pub)
            message = b'(request-target): get /note\nhost: remote.fixture.invalid\ndate: Wed, 01 Jul 2026 12:00:00 GMT'
            signature = subprocess.run(['openssl', 'dgst', '-sha256', '-sign', str(key)], input=message, capture_output=True, check=True).stdout
            headers = {'host': 'remote.fixture.invalid', 'date': 'Wed, 01 Jul 2026 12:00:00 GMT', 'Signature': 'keyId="https://fixture-v4-6-5.rustodon.invalid/actor#main-key",headers="(request-target) host date",signature="' + base64.b64encode(signature).decode() + '"'}
            handler = SimpleNamespace(path='/note', headers=headers)
            self.assertTrue(source.verified(handler))
            handler.path = '/tampered'
            self.assertFalse(source.verified(handler))
            headers['Signature'] = headers['Signature'].replace('/actor#', '/users/alice#')
            self.assertFalse(source.verified(handler))

    @unittest.skipUnless(os.environ.get('STATUS_SEARCH_PINNED_SOURCE'), 'opt-in clean pinned source contract')
    def test_pinned_search_query_contract(self):
        reference = Path(os.environ['STATUS_SEARCH_PINNED_SOURCE'])
        subprocess.run([str(ROOT / 'tools/mastodon-fixture'), 'verify-source', str(reference)], check=True, capture_output=True, timeout=30)
        actions = (reference / 'app/javascript/mastodon/actions/search.ts').read_text()
        submit = actions.split('export const submitSearch =', 1)[1].split('export const expandSearch =', 1)[0]
        for fragment in ('const signedIn = !!getState().meta.get(\'me\')', 'resolve: signedIn', 'limit: 11'):
            self.assertIn(fragment, submit)
        expand = actions.split('export const expandSearch =', 1)[1].split('export const openURL =', 1)[0]
        for fragment in ('results?.[type].length', 'limit: 10', 'offset,'):
            self.assertIn(fragment, expand)
        self.assertNotIn('resolve:', expand)
        api = (reference / 'app/javascript/mastodon/api/search.ts').read_text()
        self.assertIn("'v2/search'", api)
        rails = (reference / 'app/services/search_service.rb').read_text()
        for fragment in ('options[:type].blank? ? 0 : options[:offset].to_i', '@limit.zero?', '@offset.positive?', 'url_resource_symbol != @options[:type].to_sym', '@resolve && %r{\\Ahttps?://}', 'Chewy.enabled? && status_search? && @account.present?'):
            self.assertIn(fragment, rails)
        results = (reference / 'app/javascript/mastodon/features/search/index.tsx').read_text()
        for fragment in ("setType('statuses')", "defaultMessage='Posts'", 'results[mappedType].length > INITIAL_PAGE_LIMIT'):
            self.assertIn(fragment, results)

    def test_replacement_rejects_drift(self):
        with self.assertRaises(ValueError):
            m.replace_once('unexpected', 'old', 'new')


if __name__ == '__main__':
    unittest.main()
