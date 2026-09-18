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

`media/avatar.gif`, `media/attachment.jpg`, `media/emojo.png`,
`media/600x400.avif`, `media/600x400.heic`, `media/attachment.webm`, and
`media/boop.ogg` are copied byte-for-byte from `spec/fixtures/files/` at the
same pinned Mastodon revision. They are used by the ordinary Paperclip tests
and the bounded processor capability check so those checks do not require an
upstream checkout at runtime. Their SHA-256 digests are:

```text
963f3568ab0029445cddd70924ff4a97b72621f5554b47f7b5fb6cf87a93a5c6  media/600x400.avif
5a6fbb070a82e2101f5d592bc8f96237a0332f7dcf754d4228a54ca05f842a9d  media/600x400.heic
8b55412b52ca2cd678e65f6f1d89760fee333f488246c6b1e40a199a18be7e54  media/attachment.webm
ae0edbdb48f1d4d0529d09313deefa50ab3f0a8174ae9e23f5db4ccd95a41778  media/boop.ogg
```

Existing image dimension, animation, format, metadata, and failure-recovery
assertions are unchanged. Explicit fixture integration/differential gates may
still use the read-only upstream checkout.

The `media/capability-*` and `media/capability.{3gp,aac,asf,flac,m4a,mov,mp3,mp4,wav}`
files are synthetic 0.12–0.2 second codec checks generated with FFmpeg 9.0.1
from fixed `lavfi` sine/color sources, with metadata removed. They contain no
user data and cover each distinct advertised demux/decode path. Their SHA-256
digests are:

```text
be69f05227f9a15eeca8fc6a1fcd0274d3c3ea167dc193ade1589f5827bda338  media/capability-audio.webm
6dc937aa41ee756859ca344095577a56efbbf4476db821aa3d458b13eb778d72  media/capability-video.ogg
e2e78931e2f297498bd272b3fcc04fbda3acd97c28acbd5725cd4ac5b73bf0f0  media/capability.3gp
3ff0bdf1ca6d7bdc3dbb234b5d2c84b7a1a5af459e2926f2521c699c9c3d4c80  media/capability.aac
da32ed901f94e0ba9ff1980ea2dda989bdaa307744c8215c62f364cea1e76685  media/capability.asf
8830f40593ab1904eb0315d5beec3a4b2047c928c224482b2ab9bc6605301f69  media/capability.flac
6e9c87120e714dd1464fdb60ae797ec76c8e7d91944847decbf908929a27803f  media/capability.m4a
1464d7cb0dd13373fe7cbac4e698a9b7e19e9f8a18815bfe1ae2c0043803be6f  media/capability.mov
6d8b2ac75c029b9e2156f068360f7c2db30939c3f4839eb50d79bc975b4cea85  media/capability.mp3
6011f498b2cde3b8a03b605317a215c85c04ce2456be5a3b51226b9bb982bbcc  media/capability.mp4
ad985e94dffac23d2c4a8e8fd72764856682e8c680b89c8f7b2ddb9c1464d170  media/capability.wav
```
