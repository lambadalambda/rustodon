# Align OAuth metadata with supported response modes

## Summary

The required pinned Rails differential, unblocked by the task-owned NAS socket,
fails because Rustodon advertises only query mode while Mastodon advertises
query, fragment and form_post. Rustodon already has response-mode behavior gates.

## Acceptance Criteria

- Verify advertised modes match actually supported authorization responses.
- Add a focused metadata regression, then minimally correct discovery metadata.
- Pinned `oauth_bearer_authentication`, response-mode fixture coverage, formatting
  and strict lint pass; no speculative protocol support or grant changes.

## Evidence

- `/srv/workspaces/rustodon-audit-green/logs/differential-task-socket.log`:
  missing `response_modes_supported[1]` fragment and `[2]` form_post.
- `src/web.rs` OAuth metadata currently emits `["query"]`.
- Follow-up from required audit gate execution; no live deployment.
