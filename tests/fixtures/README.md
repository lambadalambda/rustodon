# Test-only upstream fixtures

## HTTP signature key

`http-signature-private.pem` is a **publicly known test fixture**, never an
instance credential. Do not use it outside tests.

It is extracted from Mastodon v4.6.5, revision
`1440d55b139e39ec722c2a3db7f60b66cd889048`,
`spec/requests/signature_verification_spec.rb` (Mastodon contributors,
AGPL-3.0-or-later). Only surrounding Ruby indentation was removed; the PEM
has one final newline. The existing golden signatures in
`tests/http_signatures.rs` depend on this exact key.

Keeping the fixture here lets ordinary unit/transport tests and Clippy compile
without an ignored upstream checkout. Tests that inspect actual Mastodon
source remain in the explicit pinned-source contract gate.

## Media

`media/avatar.gif`, `media/attachment.jpg`, and `media/emojo.png` are copied
byte-for-byte from `spec/fixtures/files/` at the same pinned Mastodon revision.
They are used by the ordinary Paperclip tests so those tests do not require
an upstream checkout at runtime. Their existing dimension, animation, format,
metadata, and failure-recovery assertions are unchanged. Explicit fixture
integration/differential gates may still use the read-only upstream checkout.
