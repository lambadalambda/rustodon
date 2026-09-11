import importlib.util
import io
import json
import pathlib
import tempfile
import unittest
from unittest import mock

spec = importlib.util.spec_from_file_location('peer_proxy', pathlib.Path(__file__).with_name('tls-proxy.py'))
proxy = importlib.util.module_from_spec(spec)
spec.loader.exec_module(proxy)


class AuditTests(unittest.TestCase):
    def test_records_identity_not_body_or_credentials(self):
        event = proxy.audit_event('POST', '/inbox', 'peer.invalid', 202, True,
                                  b'{"type":"Create","actor":"actor","object":{"id":"note","content":"private text","to":["https://www.w3.org/ns/activitystreams#Public"]}}')
        self.assertEqual(event, dict(method='POST', path='/inbox', host='peer.invalid',
                                     status=202, signed=True, activity='Create', activity_id=None, actor='actor',
                                     object='note', public=True, recipients=['https://www.w3.org/ns/activitystreams#Public'],
                                     outer_public=False, outer_recipients=[]))
        self.assertNotIn('private text', str(event))

    def test_private_audience_excludes_public_and_content(self):
        event = proxy.audit_event('POST', '/inbox', 'peer.invalid', 202, True,
                                  b'{"type":"Create","actor":"actor","object":{"id":"note","content":"secret","to":["recipient"],"cc":["followers"]}}')
        self.assertEqual(event['recipients'], ['recipient', 'followers'])
        self.assertFalse(event['public'])
        self.assertNotIn('secret', str(event))

    def test_like_and_undo_preserve_wire_activity_identity(self):
        like = proxy.audit_event('POST', '/inbox', 'peer.invalid', 202, True,
                                 b'{"id":"like-id","type":"Like","actor":"actor","object":"note"}')
        undo = proxy.audit_event('POST', '/inbox', 'peer.invalid', 202, True,
                                 b'{"id":"undo-id","type":"Undo","actor":"actor","object":{"id":"like-id","type":"Like","object":"note"}}')
        self.assertEqual(like['activity_id'], undo['object'])
        self.assertEqual(like['object'], 'note')
        self.assertEqual(undo['activity_id'], 'undo-id')

    def test_private_announce_envelope_is_distinct_from_public_embedded_note(self):
        event = proxy.audit_event('POST', '/inbox', 'peer.invalid', 202, True,
                                  b'{"type":"Announce","id":"boost","actor":"actor","to":["followers"],"object":{"id":"note","to":["https://www.w3.org/ns/activitystreams#Public"]}}')
        self.assertEqual(event['object'], 'note')
        self.assertEqual(event['outer_recipients'], ['followers'])
        self.assertFalse(event['outer_public'])
        self.assertTrue(event['public'])

    def test_forward_records_public_attempt_even_when_backend_fails(self):
        body = b'{"type":"Announce","id":"boost","actor":"actor","object":"note","to":["https://www.w3.org/ns/activitystreams#Public"]}'
        for failing_method in ('request', 'getresponse'):
            with self.subTest(failing_method=failing_method), tempfile.TemporaryDirectory() as root:
                handler = object.__new__(proxy.Proxy)
                handler.connection = mock.Mock()
                handler.headers = {'Host': 'peer.invalid', 'Content-Length': str(len(body)),
                                   'Signature': 'not-recorded'}
                handler.rfile = io.BytesIO(body)
                handler.command = 'POST'
                handler.path = '/inbox'
                connection = mock.Mock()
                getattr(connection, failing_method).side_effect = ConnectionResetError('backend reset')
                with mock.patch.multiple(proxy, root=root, name='peer.invalid', backend=1234, create=True), \
                        mock.patch.object(proxy.http.client, 'HTTPConnection', return_value=connection):
                    with self.assertRaises(ConnectionResetError):
                        handler.forward()
                connection.close.assert_called_once_with()
                events = [json.loads(line) for line in pathlib.Path(root, 'peer.invalid.jsonl').read_text().splitlines()]
                self.assertEqual(len(events), 1)
                self.assertEqual(events[0], proxy.audit_event('POST', '/inbox', 'peer.invalid', None, True, body))
                self.assertTrue(events[0]['outer_public'])
                self.assertNotIn('not-recorded', str(events))

    def test_non_activity_and_malformed_json_are_safe(self):
        for body in (b'', b'bad json', b'[]', b'{"object":[]}', b'{"object":{"to":null}}'):
            event = proxy.audit_event('GET', '/actor', 'peer.invalid', 200, False, body)
            self.assertEqual(event['method'], 'GET')
            self.assertFalse(event['public'])


if __name__ == '__main__':
    unittest.main()
