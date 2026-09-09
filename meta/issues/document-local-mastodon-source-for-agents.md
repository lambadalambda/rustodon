# Document local Mastodon source for agents

## Summary

Direct agents to the existing pinned Mastodon checkout instead of fetching the
same source from GitHub.

## Requirements

- Document the local checkout path and pinned revision in repository agent guidance.
- Tell agents not to clone, fetch, pull, or modify the checkout.
- Require Mastodon-related subagent prompts to include the local checkout path.

## Acceptance Criteria

- Repository agent guidance points to the verified local Mastodon 4.6.5 checkout.
- The maintenance issue is archived after the guidance is added.
