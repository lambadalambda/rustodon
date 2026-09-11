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
