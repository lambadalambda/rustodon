#!/usr/bin/env python3
"""Offline contracts for the opt-in remote browser fixture (no browser/DB)."""
import importlib.util
from pathlib import Path
import unittest
from unittest.mock import patch
import tempfile
import json

spec = importlib.util.spec_from_file_location('fixture', Path(__file__).parents[1] / 'remote-browser/fixture.py')
fixture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixture)
spec = importlib.util.spec_from_file_location('ui', Path(__file__).parents[1] / 'remote-browser/ui.py')
ui = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ui)


class Contracts(unittest.TestCase):
    def test_notes_retain_advertised_family_and_origin(self):
        for kind, (_, mime) in fixture.MEDIA.items():
            activity = fixture.activity(kind)
            note = activity['object']
            self.assertEqual(activity['type'], 'Create')
            self.assertEqual(note['type'], 'Note')
            self.assertEqual(note['attributedTo'], fixture.ACTOR)
            self.assertEqual(note['attachment'][0]['mediaType'], mime)
            self.assertEqual(note['attachment'][0]['url'], fixture.ORIGIN + '/media/' + kind)
            self.assertIn(fixture.PUBLIC, note['to'])
            self.assertNotIn('preview', note['attachment'][0])

    def test_network_records_strip_tokens_and_headers(self):
        records = ui.sanitize_requests([{'url': 'wss://fixture.invalid/stream?access_token=secret#secret',
                                        'headers': {'Authorization': 'secret'},
                                        'responseHeaders': {'set-cookie': 'secret'},
                                        'method': 'GET', 'resourceType': 'WebSocket',
                                        'status': 101, 'mimeType': 'application/json'}])
        self.assertNotIn('secret', repr(records))
        self.assertEqual(records[0]['url'], 'wss://fixture.invalid/stream')
        self.assertEqual(records[0]['status'], 101)

    def test_reloaded_audio_rejects_preview_or_small(self):
        good = {'id': '1', 'type': 'audio', 'url': ui.BASE + '/cached.mp3',
                'preview_url': None, 'meta': {}}
        ui.assert_media('audio', good)
        for bad in [dict(good, preview_url=ui.BASE + '/fake.png'),
                    dict(good, meta={'small': {}})]:
            with self.assertRaises(AssertionError):
                ui.assert_media('audio', bad)

    def test_reload_selects_current_frontend_response_not_live_record(self):
        current = {'id': '1', 'type': 'audio', 'url': ui.BASE + '/cached.mp3',
                   'preview_url': None, 'meta': {'small': {}}}
        with self.assertRaises(AssertionError):
            ui.reload_media('audio', 'status-id', '1',
                            [{'id': 'status-id', 'media': [current]}])
        with self.assertRaises(AssertionError):
            ui.reload_media('audio', 'status-id', '1', [])

    def test_stage_network_needs_own_positive_and_rejects_origin(self):
        media = {'url': ui.BASE + '/cached.mp3', 'preview_url': None}
        success = {'url': media['url'], 'status': 206}
        ui.assert_network([success], [('audio', media)])
        for records in [[], [{'url': ui.BASE + '/other', 'status': 200}],
                        [success, {'url': fixture.ORIGIN + '/media/audio', 'status': 200}]]:
            with self.assertRaises(AssertionError):
                ui.assert_network(records, [('audio', media)])

    def test_stage_clear_preserves_aggregate_and_checks_dom_each_time(self):
        raw = json.dumps({'data': {'requests': [{'url': ui.BASE + '/local', 'status': 200}]}})
        with tempfile.TemporaryDirectory() as directory, patch.object(ui, 'E', Path(directory)), \
                patch.object(ui, 'ab', return_value=raw) as browser, \
                patch.object(ui, 'js', return_value=[]) as dom:
            ui.network('video-reload')
            ui.network('audio-reload')
            ledger = json.loads((Path(directory) / 'aggregate-network.json').read_text())
            self.assertEqual([r['stage'] for r in ledger], ['video-reload', 'audio-reload'])
            self.assertEqual(dom.call_count, 2)
            self.assertEqual(sum(c.args == ('network', 'requests', '--clear')
                                 for c in browser.call_args_list), 2)
            dom.return_value = [fixture.ORIGIN + '/media/still']
            browser.reset_mock()
            with self.assertRaises(AssertionError):
                ui.network('still-reload')
            self.assertFalse(any(c.args == ('network', 'requests', '--clear')
                                 for c in browser.call_args_list))

    def test_unknown_kind_fails_closed(self):
        with self.assertRaises(KeyError):
            fixture.activity('../secret')

    def test_audit_has_no_credentials(self):
        event = fixture.audit('/media/video', True, 'held')
        self.assertEqual(event, {'path': '/media/video', 'signed': True, 'phase': 'held'})


if __name__ == '__main__':
    unittest.main()
