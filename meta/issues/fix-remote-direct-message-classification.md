# Fix remote direct-message versus limited classification

## Summary

Remote Note classification never returns direct visibility 3; explicitly mentioned DMs become limited, serialize as private and miss direct-only conversation handling.

## Requirements

- Port pinned Mastodon explicit-mention versus silent-audience cases using real restricted-writer processing.
- Preserve access denial, provenance, followers/private/public semantics and silent-recipient behavior.
- Reuse existing peer privacy scenario; do not broadly rewrite stored historical rows.

## Acceptance Criteria

- Red regression proves explicitly mentioned direct messages currently classify incorrectly.
- Green checks cover direct versus silent-limited, REST visibility, conversation effects, unauthorized access and replay behavior.
- Existing peer direct scenario is executed later on combined source, or remains explicitly blocked.

## Notes

- Source: [test coverage audit](../test-coverage-audit.md).
- Implementation plan recorded on 2026-09-12; no live deployment is implied.

## Progress — open, awaiting parent green/review

- Phase 1 added `tests/workers/direct_visibility.rs` and its worker-suite module
  declaration. The five tests use the real restricted writer, ingress/notification
  workers and REST router; they cover explicit `to`/`cc`, silent-only and mixed
  recipients, public/unlisted/followers controls, conversations, access denial,
  stream privacy and replay. Existing peer privacy coverage is unchanged.
- Parent-reported isolated-worker behavioral red:
  **3 passed, 2 failed**; the historical external artifact is not in the repository.
  Both explicitly mentioned `to`/`cc` cases stored visibility **4**, expected **3**.
  The tests compiled; this worktree did not execute or independently inspect the
  isolated-worker log. The parent confirmed the pinned Mastodon 4.6.5 Create lines 126–150
  explicit-versus-silent contract before drafting.
- Phase 2 refines only newly created limited-candidate Notes after local mentions
  and the implicit inbox recipient have been resolved. A resolved explicit
  recipient with no silent recipients becomes direct. Known remote audience
  accounts also participate in this classification, without fetching unknown
  accounts or treating unknown/collection URIs as silent recipients. Initial
  public, unlisted and followers-only classification is unchanged.
- The refinement occurs inside the existing Create transaction, before status
  counts, notification intents and stream intents. Existing visibility-3-only
  notification/conversation handling and REST serialization are reused; no new
  conversation implementation, grants, schema changes or historical-row rewrite.
  Provenance, relevance and existing-row replay checks are unchanged.
- Parent owns green, formatting/lint and independent implementation review. No
  local/isolated worker builds, tests, formatting, lint or commits were run in this worktree;
  regression assertions were not weakened. The parent's separate default-profile
  gating fix (`1a0b9c6`) is not duplicated here.
- Pending command in the parent's prepared disposable fixture:
  `cargo test --locked --features test-support --test workers direct_visibility:: -- --ignored --nocapture --test-threads=1`
  (five tests). The existing peer privacy scenario still requires later combined
  execution; it is not certified by the synthetic worker suite.
- Additional behavior not specifically exercised by the focused matrix: implicit
  inbox recipients absent from `to`/`cc`, actor-URI aliases, known remote silent
  recipients, unknown audience URIs, and direct replies sharing an existing
  conversation. These remain review/coverage caveats, not claimed green evidence.

## Completion evidence (2026-09-12)

- Baseline: five tests, three passed and two explicit-recipient cases failed
  (stored visibility 4, expected 3); the historical external artifact is not in the repository.
- Combined isolated-worker suite: 85 passed, including all five direct visibility
  regressions; the historical external artifact is not in the repository.
- Default/all-feature debug, release all-feature, formatting and strict Clippy
  pass on the combined fixes. Independent correctness/privacy/DRY review found
  no blockers. No schema, grants, historical replay, or live deployment.
- Follow-up coverage, not expanded here: implicit inbox targets, actor aliases,
  known remote silent recipients, and empty/unknown resolved audiences.
