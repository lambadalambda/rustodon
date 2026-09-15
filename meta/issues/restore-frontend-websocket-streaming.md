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
