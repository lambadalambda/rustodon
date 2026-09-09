# Implement shared REST protocol contracts

## Summary

Centralize HTTP behavior shared by every supported Mastodon REST endpoint.

## Requirements

- Implement CORS and OPTIONS, content types, cache and Vary headers, request
  limits, parameter errors, scope declarations, error envelopes, and reusable
  endpoint-specific pagination.
- Maintain an explicit implemented and disabled route inventory.

## Acceptance Criteria

- Differential cases cover anonymous, bearer, wrong-scope, malformed, and
  cross-origin requests without advertising unsupported routes.
