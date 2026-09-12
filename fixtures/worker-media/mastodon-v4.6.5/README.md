# Pinned worker media fixtures

These three files are copied byte-for-byte from `spec/fixtures/files/` in
Mastodon **v4.6.5**, pinned source revision
`1440d55b139e39ec722c2a3db7f60b66cd889048`.

Extraction source: the already-cached official image
`ghcr.io/mastodon/mastodon@sha256:696439e1ada71d0cf3d51d4d6a4744d6e40b57aafa64980b18f4d3b78230d0cf`,
under `/opt/mastodon/spec/fixtures/files/`. `LICENSE` is copied from
`/opt/mastodon/LICENSE` in the same image (GNU AGPL v3).
No new upstream checkout or network download was used.

`SHA256SUMS` records their exact SHA-256 digests. Run
`tools/verify-worker-media` to validate both inventory and pinned bytes; the
worker fixture harness runs this preflight before database setup. These are
small test inputs, not generated application artifacts or schema fixtures.

This directory is **not** an upstream Git checkout. Source-contract and
reference-oracle gates retain their separate full-source verification.
