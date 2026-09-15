# Restore frontend WebSocket streaming connection

## Summary

The Mastodon frontend repeatedly fails to connect to the Rustodon WebSocket endpoint at `/api/v1/streaming/`, preventing live updates.

## Requirements

- Reproduce the failed WebSocket handshake through the normal public route.
- Isolate whether the failure is caused by routing/proxy behavior, authentication, protocol handling, or frontend compatibility.
- Add regression coverage before the smallest product fix when the cause is in Rustodon.
- Preserve existing streaming authorization, subscription, and resource-safety guarantees.

## Acceptance Criteria

- The normal Mastodon frontend can establish its WebSocket connection without repeated console failures.
- Existing authenticated streaming behavior and tests continue to pass.
- Public deployment verification confirms a successful WebSocket upgrade and stable service health.

## Notes

- Keep deployment-specific evidence in the private operations note.

## Reproduction

- The bundled Mastodon frontend opens the socket with
  `new WebSocket(url, accessToken)`, which sends the OAuth token as the requested
  WebSocket subprotocol while leaving the URL query empty.
- Rustodon accepts that token and returns HTTP 101, but its handshake does not
  return `Sec-WebSocket-Protocol`. A minimal Chromium probe reproduced the same
  connection error when a server omitted the requested protocol and opened
  successfully when the server echoed it.
- Bearer-header and query-token handshakes already upgrade successfully. The
  failure is therefore WebSocket subprotocol negotiation, not Caddy routing or
  general token validity.

## Implementation

- Added a focused handshake regression against the frontend's public
  `/api/v1/streaming/` route using the fixture OAuth token as the sole requested
  WebSocket protocol. The test subscribes to the user stream and receives an
  expected event over that connection, then verifies a query-token control
  upgrades without a `Sec-WebSocket-Protocol` response.
- A single streaming credential-selection helper now returns both the synthesized
  authentication headers and the optional protocol to select, keeping explicit
  Bearer > non-empty query token > protocol-token precedence atomic. The handler
  selects that exact offered token only when protocol-token authentication won;
  repeated authentication, scopes, token identity, and stream authorization are
  unchanged.
- Registered the focused regression in the operational-schema integration lane;
  the issue remains open pending public deployment/browser verification.

## Verification

- Red: the focused ignored fixture test failed before the product change with
  `Protocol(SecWebSocketSubProtocolError(NoSubProtocol))`, proving the HTTP 101
  response omitted the protocol expected by the client.
- Green: `cargo test --locked --test streaming
  websocket_protocol_token_is_echoed_in_handshake -- --ignored --exact
  --nocapture` passed in a disposable Linux/PostgreSQL fixture (`1 passed`),
  including a protocol-authenticated subscription/event delivery and a
  query-token upgrade with no selected protocol.
- `cargo test --locked --lib
  web::tests::streaming_authentication_accepts_pinned_token_locations -- --exact
  --nocapture` passed in Linux (`1 passed`), asserting explicit Bearer > non-empty
  query > protocol precedence, empty-query fallback, and no selected protocol
  when a distinct offered protocol loses to query authentication.
- `cargo fmt --check` and `git diff --check` passed.
