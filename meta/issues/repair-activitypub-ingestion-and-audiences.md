# Repair ActivityPub ingestion and audiences

## Summary

Accept ordinary serializer-produced documents and preserve the audience fields needed for fresh-account federation.

## Requirements

- Accept nullable no-CW Note summaries and standard actor Image.url media shapes through validation and persistence.
- Persist remote followers collection URLs during initial actor resolution and refresh.
- Derive canonical local followers audiences for Announce rather than trusting an empty stored URL.
- Implement nullable Notes, actor media, remote actor persistence, and local Announce audiences as small separate commits.

## Acceptance Criteria

- Serializer-to-inbox round trips and worker tests cover no-CW Create/Update and full actor profile Updates.
- Fresh actor discovery, Follow/Accept, and shared-inbox private Note ingestion work without pre-seeding followers_url.
- A freshly created local account emits a correctly addressed followers-only boost and a real peer ingests it for its follower.

## Notes

- Findings R04, R05, R06, R09 in [the essential feature parity review](../essential-feature-parity-review.md), baseline `1861d33`.
- Review-only discovery: no implementation fix is included in the review commit. End-to-end scenarios remain to be executed unless the report explicitly records a parser reproduction.

## R04 nullable Note summaries — verified

- Accept JSON `null` as an absent content warning while retaining rejection of
  non-string/non-null values and oversized strings.
- Serializer-produced Create and Update round trips cover empty and nonempty
  warnings. Existing durable worker fixtures now exercise null summaries for
  Create/Update, fetched URI-only Notes, and fetched/embedded Announce targets.
- Secunda RED: serializer roundtrip failed with `Activity` before the fix.
  GREEN: both targeted regressions; `tools/mastodon-fixture worker-test` **49/49**;
  all-feature library tests **241 passed, 2 ignored**; formatting and
  all-target/all-feature Clippy with warnings denied passed.
- Logs: `/home/lain/rustodon-parity/null-summary-{red,green,workers,library,clippy}.log`.
  Independent review found no blockers. R05/R06/R09 and real-peer acceptance
  remain open; this does not claim complete ingestion/audience parity.

## R06 actor Image.url Updates — verified

- Inbox validation and persistence share a media-only URI extractor. Standard
  `Image.url` wins over image identity; legacy string/id/href values retain the
  existing URI checks. Actor identity URI parsing is unchanged.
- Serializer-produced full actor Update now roundtrips through the inbox. Tests
  cover invalid schemes/shapes and URL precedence; the restored actor lifecycle
  asserts profile text and both persisted media URLs before deletion.
- Secunda RED: serialized Update rejected with `Activity`. A second run with
  validation fixed but persistence unchanged proved avatar `None` and header
  empty despite updated profile text. Its early assertion also left fixture data
  that caused two later failures; all disappeared in GREEN.
- GREEN: library **242 passed, 2 ignored**, restored worker **49/49**, formatting,
  and all-target/all-feature Clippy with warnings denied. Logs beneath
  `/home/lain/rustodon-parity/`: `actor-media-red.log`,
  `actor-media-worker-red.log`, `actor-media-worker-green.log`,
  `actor-media-library.log`, `actor-media-clippy.log`.
- Independent review's test-import blocker was fixed and re-reviewed; no remaining
  blockers. These gates preceded integration of R01; combined gates follow.
  Real-peer profile convergence and the remaining audience criteria stay open.
