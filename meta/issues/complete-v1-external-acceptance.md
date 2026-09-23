# Complete v1 external acceptance

## Summary

The v1 implementation issues are code complete. What remains is evidence that
needs a real client, peer, browser or the production instance. This issue
collects that evidence in one checklist. It replaces the remaining-work notes of
the archived parent issues listed below.

## Requirements

- [ ] One clean final-tree sweep of the named gates on the isolated worker:
      ordinary/profile, strict Clippy, cargo-deny, schema-read, operational
      schema, worker, startup, preflight, cutover, differential and browser
      ([run-essential-parity-gates-on-isolated-worker](run-essential-parity-gates-on-isolated-worker.md)).
- [ ] The sweep covers the implemented but unproven lanes of
      [polls](support-poll-voting-and-refresh.md),
      [timeline streams](stream-public-hashtag-list-timelines.md) and
      [account-stats self-heal](self-heal-missing-account-statistics.md).
- [ ] Final-tree Mastodon peer scenarios, including a reply scenario and
      duplicate/ordering convergence
      ([prove-mastodon-peer-federation-compatibility](prove-mastodon-peer-federation-compatibility.md),
      [add-isolated-federation-peer-tests](add-isolated-federation-peer-tests.md)).
- [ ] A recorded mobile-client run against rustodon.social (ACCEPT-04).
- [ ] Broader browser forms and interactive TOTP login in the pinned browser.
- [ ] A production cutover and rollback rehearsal, including database and media
      reopen by Mastodon.
- [ ] `docs/v1-acceptance-matrix.md` rows updated from the results above.

## Acceptance Criteria

- Every box above is checked with a pointer to the run, or explicitly moved out
  of v1 in `docs/v1-scope.md`.

## Notes

- Folded parents (archived 2026-09-23): implement-v1-policy-and-abuse-controls,
  handle-activitypub-relationship-activities, handle-activitypub-note-activities,
  implement-outbound-activitypub-delivery, serve-mastodon-web-client,
  implement-minimal-account-ui, build-v1-acceptance-matrix, harden-v1-release.
- Durability beyond v1 is tracked in
  [prove-post-v1-durability](prove-post-v1-durability.md).
