# Provision pinned worker media fixtures explicitly

## Summary

Six worker tests depend on attachment.gif, avatar.gif and attachment.jpg from an absent upstream source path on fresh CI/NAS workspaces.

## Requirements

- Supply only exact verified fixture assets from the existing pinned source/image, with hashes and clear provenance.
- Do not fetch or replace the canonical upstream checkout merely for these assets.
- Fail early and clearly on absent/mismatched dependencies; keep source-contract verification separate.

## Acceptance Criteria

- A clean workspace reproduces the missing prerequisite and then passes prerequisite checks without a full upstream checkout.
- Full worker fixture suite passes and corrupted fixture inputs fail closed.

## Notes

- Source: [test coverage audit](../test-coverage-audit.md).
- User authorized this implementation plan on 2026-09-12; no live deployment is implied.
