# Ignore unsigned HTTP signature expiry

## Summary

The `expires=` parameter of an HTTP signature is not covered by the signature,
but Rustodon uses it to extend the validity window. A relaying server can
replay a captured request with `expires` set far in the future and extend the
replay window from about 65 minutes to about 13 hours. Mastodon has the same
gap; the activity-ID dedup limits the impact.

## Requirements

- An unsigned `expires` may only shorten the default `Date` window (Mastodon
  behavior, covered by an existing test); it can never extend it.

## Acceptance Criteria

- A unit test proves an unsigned far-future `expires` does not extend the window.
- Existing signature tests pass.

## Done 2026-09-23

Red/green on the NAS PG14 fixture; see DEVLOG 2026-09-23.
