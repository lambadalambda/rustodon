# Diagnose missing remote animated media

## Summary

The user reports that a remote GIF does not display. The affected post URL is not yet identified.

## Requirements

- Identify the affected attachment and distinguish download, format processing, persistence and serving failures using read-only diagnostics first.
- Do not display private post bodies or credentials, or alter live attachments to mask the problem.
- Reproduce an identified defect on Secunda before any minimal independently reviewed fix.

## Acceptance Criteria

- Establish the failure boundary for the reported attachment.
- Any repair has applicable regression coverage and checks on Secunda.
- Verify the recovered attachment is served in a frontend-compatible representation, or record the remaining blocker.

## Notes

- Awaiting a user-provided post link to identify the exact affected media.
- Separate from [remote profile media](diagnose-missing-remote-avatar-and-banner.md) and [reply-thread persistence](repair-remote-reply-thread-persistence.md).
