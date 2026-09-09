# Serve the Mastodon web client

## Summary

Package and serve the exact Mastodon 4.6.5 frontend contract.

## Requirements

- Reproducibly build/checksum Vite assets from the local pinned source and
  serve chunks, themes, locales, icons, uploads, and `/sw.js`.
- Render the shell, initial-state JSON, CSRF data, initial path, VAPID metadata,
  and `#mastodon` mount while advertising excluded features as disabled.

## Progress

- Built the pinned Vite production output and recorded the release, revision,
  tool versions, manifests, and SHA-256 checksums in `public/packs`.
- Added hashed asset serving, SPA deep-link fallback, `/manifest`, `/sw.js`,
  favicon serving, public service-worker assets, CSRF/VAPID metadata, and
  escaped initial state rendering.
- Hydrated authenticated browser sessions with their credential account,
  preferences, role, and session OAuth token.
- Covered the complete pinned React web-app route inventory, hardened HTML
  responses with security headers and a nonce-backed CSP, rejected revoked or
  expired browser-session tokens, and restricted remote media proxy MIME types.
- Added unit coverage, authenticated-shell coverage, and the guarded
  `local_web_client_shell` router case.
- Added the opt-in `mise run browser-integration` gate. It runs the real web
  process in the cutover fixture and drives Chromium through `agent-browser` to
  verify the React mount, app body, SPA deep-link navigation, and page errors;
  optional WebM and PNG paths retain operator evidence.
- The browser-backed startup/navigation run passes with a PNG screenshot on the
  current Linux host. WebM capture is available when `ffmpeg` is installed;
  mobile-client recording remains external acceptance work.

## Acceptance Criteria

- The pinned frontend and a recorded mobile-client version complete startup and
  normal navigation against Rustodon. Browser-backed startup/navigation is
  locally proven; the mobile-client recording remains outstanding.
- Full Rails `Web::Setting` persistence, Web Push subscription management, and
  non-English locale parity remain outside the current v1 web-client slice;
  essential account settings continue through the Rust REST surface.
