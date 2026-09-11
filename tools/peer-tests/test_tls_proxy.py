import importlib.util
import pathlib
import unittest

spec = importlib.util.spec_from_file_location('peer_proxy', pathlib.Path(__file__).with_name('tls-proxy.py'))
proxy = importlib.util.module_from_spec(spec)
spec.loader.exec_module(proxy)


class AuditTests(unittest.TestCase):
    def test_records_identity_not_body_or_credentials(self):
        event = proxy.audit_event('POST', '/inbox', 'peer.invalid', 202, True,
                                  b'{"type":"Create","actor":"actor","object":{"id":"note","content":"private text","to":["https://www.w3.org/ns/activitystreams#Public"]}}')
        self.assertEqual(event, dict(method='POST', path='/inbox', host='peer.invalid',
                                     status=202, signed=True, activity='Create', actor='actor',
                                     object='note', public=True, recipients=['https://www.w3.org/ns/activitystreams#Public']))
        self.assertNotIn('private text', str(event))

    def test_private_audience_excludes_public_and_content(self):
        event = proxy.audit_event('POST', '/inbox', 'peer.invalid', 202, True,
                                  b'{"type":"Create","actor":"actor","object":{"id":"note","content":"secret","to":["recipient"],"cc":["followers"]}}')
        self.assertEqual(event['recipients'], ['recipient', 'followers'])
        self.assertFalse(event['public'])
        self.assertNotIn('secret', str(event))

    def test_non_activity_and_malformed_json_are_safe(self):
        for body in (b'', b'bad json', b'[]', b'{"object":[]}', b'{"object":{"to":null}}'):
            event = proxy.audit_event('GET', '/actor', 'peer.invalid', 200, False, body)
            self.assertEqual(event['method'], 'GET')
            self.assertFalse(event['public'])


if __name__ == '__main__':
    unittest.main()
