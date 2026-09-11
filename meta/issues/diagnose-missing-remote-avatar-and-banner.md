# Diagnose missing remote avatar and banner

## Summary

The user reports that followed remote account `lain@lain.com` lacks its avatar and banner on the local instance.

## Requirements

- Distinguish remote actor discovery/profile metadata problems from media download, persistence or rendering failures.
- Use read-only live diagnostics first; do not display credentials or private content.
- Reproduce any defect on Secunda before a minimal independently reviewed fix; preserve existing remote identity and moderation settings.

## Acceptance Criteria

- Identify the failing boundary for both avatar and banner.
- Applicable regression checks pass for any repair.
- Verify profile metadata and rendered media after recovery, or record the remaining external blocker.

## Notes

- Reported while investigating [reply-thread persistence](repair-remote-reply-thread-persistence.md); keep the repairs topical.
