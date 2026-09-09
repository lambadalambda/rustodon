# Prove concurrent status interactions

## Summary

Prove that concurrent bookmark, favourite, and reblog requests converge on the
same Mastodon interaction state.

## Requirements

- Exercise concurrent create and remove requests for bookmarks and favourites.
- Exercise concurrent reblog creation and removal against the original status.
- Compare stable final rows, counters, and compatible HTTP results, then restore
  all interaction, conversation, account, and notification state.

## Acceptance Criteria

- Guarded Rails-versus-Rust differential coverage runs the concurrent cases,
  accepts documented unique-validation loser responses, and proves no duplicate
  interaction rows or counter drift.

## Notes

- Complete notification/outbox delivery and remaining interaction APIs remain
  covered by the parent status-social-interactions issue.
- The differential fixture intentionally has no Sidekiq consumer. The proof
  snapshots notifications and notification requests for both fixture accounts,
  restores every interaction-related table after each case, and applies the
  database effects of Rails' queued favourite/reblog removal workers before
  comparing stable rows and counters.
