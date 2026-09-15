# Support quote posts from the bundled frontend

## Summary

Mastodon 4.6.5 exposes a prominent “Boost or quote” action, but Rustodon only reads and federates preexisting quote rows. The frontend sends `quoted_status_id` during status creation, but Rustodon ignores it and publishes an otherwise-valid submission as an ordinary standalone post without the quoted status.

## Requirements

- Accept the bundled frontend's quote-status creation parameters and persist Mastodon-compatible quote state atomically with the new status.
- Enforce quoted-status visibility, blocks, lifecycle, approval policy, and self/remote authorization rules.
- Emit the required local stream, notification, counter, and durable ActivityPub quote/authorization effects.
- Reject unsupported or invalid quote requests clearly; never silently publish the user's commentary without its intended quote.
- Preserve existing quote read/serialization and deletion/update behavior.

## Acceptance Criteria

- Choosing Quote in the bundled frontend, composing text, and submitting produces a status that visibly contains the intended quoted status after response and reload.
- Remote and approval-required quote paths converge through the supported ActivityPub representation.
- Hidden/deleted/blocked/denied targets cannot be quoted or disclosed.
- Differential, worker, and browser coverage includes create, approval/denial, update, delete, and failure rollback.

## Evidence

- Mastodon 4.6.5's bundled frontend includes `quoted_status_id` in the `POST /api/v1/statuses` payload when the user submits a quote.
- Project scope and status-write acceptance explicitly defer quote creation while retaining quote reads and federation for existing data.
- Rustodon's status-create handler parses text, media, visibility, language, sensitivity, reply target, and quote policy, but not `quoted_status_id`. The unknown field is ignored and excluded from both the write and idempotency fingerprint, so an otherwise-valid submission is created without its intended quote.
