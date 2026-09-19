# Accept local hashtag controls in browser and Rails differential

## Summary

Bounded acceptance of `8e22f52`, child of [hashtag controls](support-hashtag-and-featured-tag-controls.md). Parent source review round 2 approved; this task changes harness/evidence only, not application code.

## Requirements

- Reuse existing harnesses and verified read-only Mastodon 4.6.5 `1440d55b139e39ec722c2a3db7f60b66cd889048`.
- Real pinned-frontend clicks: header/history, Follow/Unfollow, Feature/Unfeature, profile featured-tag add/remove, reload persistence/public profile, representative home-follow inclusion. Record calls without replacing UI mutations with API calls; SQL fixture setup is allowed.
- Actual pinned Rails responses versus Rust for supported local methods, normalization, duplicates, limits, owner IDs and scopes. History compares meaningful shape/current counts separately: current-DB aggregation intentionally differs from Redis activity/retention; never normalize it to synthetic zero parity.
- Task-owned PG14, independent Rails/Rust databases/media/keys/ports, restricted Rust roles; serial bounded resources. No internet federation, production operations, deployment or push. Peer AddHashtag/RemoveHashtag remains deferred.
- Report ordinary UI/validation mismatches before expanding fix scope. At most two substantive fix/review rounds; independent harness review. Leave changes uncommitted.

## Acceptance Criteria

- [ ] Focused browser controller completes uninterrupted; sanitized actual-click/call evidence retained.
- [ ] Focused differential executes against actual pinned Rails and records supported local cases.
- [ ] Independent harness review and task-owned resource teardown recorded.

## Notes

Created/indexed before harness work. Missing Rails prerequisites are an evidence boundary: keep this and parent open, do not invent parity. Existing home websocket test evidence is not a substitute for representative browser acceptance. No full peer/release matrix required.

## 2026-09-19 — Blocked acceptance on 8e22f52

- Two fresh serial NAS PG14/pinned-browser attempts. First corrected one harness selector (public tag is a button, not a link); second controller passed real header/history, Follow/Unfollow, reload state, representative home inclusion/removal, Feature/Unfeature and public-profile presence/removal.
- **Ordinary UI blocker:** profile featured-tag editor requests `GET /api/v1/profile` → **404**. Suggestion click `POST /api/v1/featured_tags` → **200**, but the profile model never loads, so the item/Delete control is absent even after reload. Actual pinned frontend `apiGetProfile` and `profile_edit` reducer confirm this dependency. No API deletion replacement or application fix attempted. Pause for a bounded profile-read scope decision.
- Profile remove/reload/public-profile-after-add suffix remains unexecuted. Controller suffix also requires review of its collection-read expectation against the consolidated profile API. Current partial run is **not** a passing uninterrupted full controller.
- Exact Rails child image is available on NAS, but differential not run; no new selector/Rails database/media clone created before blocker pause. No synthetic parity claim. Normalization/duplicate/limit/owner-ID/scope matrix remains required.
- Offline adapter test RED → 1 passed; source revision/cleanliness and pinned assets verified. Restricted-role proof retained. Internal network, no workers/peer routes/published host ports. Task containers/PG volume/network/media/keys/env/build output removed and absence checked.
- Independent delegation blocked by nesting limit; parent harness review pending. No app edits, commits, push, production or deployment. Parent and this issue remain open.
- Harness/run details: [`tools/hashtag-controls-browser/README.md`](../../tools/hashtag-controls-browser/README.md). Sanitized actual-call/DOM/screenshots and matching executed hashes: ignored `target/hashtag-browser-acceptance-evidence/`; NAS workspace `rustodon-hashtag-browser-8e22f52-alice`. History is nonzero current-DB aggregation, not Redis activity/retention parity.

