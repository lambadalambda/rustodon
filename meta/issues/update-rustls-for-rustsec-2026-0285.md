# Update rustls for RUSTSEC-2026-0285

## Summary

`cargo deny check` fails: rustls 0.23.43 accepts TLS 1.3 handshake messages
across encryption level boundaries (RUSTSEC-2026-0285). Rustodon uses rustls
for outbound federation, SMTP and database TLS.

## Requirements

- Update rustls to >= 0.23.45 in `Cargo.lock` only.

## Acceptance Criteria

- `cargo deny --locked check` passes.
- Ordinary tests and strict Clippy pass.

## Done 2026-09-23

rustls 0.23.45. On the NAS: cargo-deny ok, strict Clippy clean, ordinary tests
pass (apart from the root-only preflight case, which passes with `--cap-drop=all`).
