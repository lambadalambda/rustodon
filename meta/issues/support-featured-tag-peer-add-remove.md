# Support featured-tag peer AddHashtag/RemoveHashtag

## Summary

Track the explicitly deferred peer projection from the completed
[ordinary local hashtag controls](support-hashtag-and-featured-tag-controls.md).
No existing narrow peer issue covered this work at local acceptance closure.
This is tracking only, not an implementation or new acceptance requirement for
that completed local scope.

## Requirements

- Inspect pinned Mastodon 4.6.5 AddHashtag/RemoveHashtag behavior and establish the
  minimum ordinary peer contract before changing protocol or durable jobs.
- Preserve existing signature, SSRF, privacy and delivery boundaries.
- Keep Redis history/retention equivalence and unrelated peer work out of scope.

## Acceptance Criteria

- Focused local regressions and a bounded disposable pinned-peer run demonstrate
  featured-tag add/remove convergence, with actual peer state and delivery evidence.
- Independent review approves implementation and harness before closure.

## Evidence boundary

Current local API/browser acceptance does not establish peer delivery. No new
implementation, peer fixture execution, production access or deployment occurred
when this issue was created. Coordinate with the existing
[peer compatibility umbrella](prove-mastodon-peer-federation-compatibility.md).
