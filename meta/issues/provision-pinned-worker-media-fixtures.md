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

## Completion evidence (2026-09-12)

- Vendored exactly three assets and upstream license from the cached pinned
  Mastodon image, with independent hardcoded hashes and provenance README.
- Prerequisite regression failed on old missing-source paths, then passed valid,
  corrupted, absent-file, and incomplete-manifest checks on NAS without a source
  checkout. Logs: `rustodon-audit-main/logs/worker-media-{red,green}.log`.
- Full combined worker suite: **85 passed**, including all seven vendored reads
  and the new direct/semantic regressions. Log:
  `/srv/workspaces/rustodon-audit-green/logs/workers.log`.
- Independent review found no blockers; full-source contract checks remain
  separate. No live instance or reference checkout changed.
