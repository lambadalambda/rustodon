# Accept local hashtag controls in browser and Rails differential

## Summary

Bounded acceptance of local controls `8e22f52` plus reviewed profile-read prerequisite `bd01acae2bc4e1b8a75bd95648e216535c790330`, child of [hashtag controls](support-hashtag-and-featured-tag-controls.md). This task changes harness/evidence only, not application code.

## Requirements

- Reuse existing harnesses and verified read-only Mastodon 4.6.5 `1440d55b139e39ec722c2a3db7f60b66cd889048`.
- Real pinned-frontend clicks: header/history, Follow/Unfollow, Feature/Unfeature, profile featured-tag add/remove, reload persistence/public profile, representative home-follow inclusion. Record calls without replacing UI mutations with API calls; SQL fixture setup is allowed.
- Actual pinned Rails responses versus Rust for supported local methods, normalization, duplicates, limits, owner IDs and scopes. History compares meaningful shape/current counts separately: current-DB aggregation intentionally differs from Redis activity/retention; never normalize it to synthetic zero parity.
- Task-owned PG14, independent Rails/Rust databases/media/keys/ports, restricted Rust roles; serial bounded resources. No internet federation, production operations, deployment or push. Peer AddHashtag/RemoveHashtag remains deferred.
- Report ordinary UI/validation mismatches before expanding fix scope. At most two substantive fix/review rounds; independent harness review. Leave changes uncommitted.

## Acceptance Criteria

- [x] Focused browser controller completes uninterrupted; sanitized actual-click/call evidence retained.
- [x] Focused actual pinned Rails differential accepted for bounded ordinary compatibility; strict response differences dispositioned without altering the failing result.
- [x] Independent harness review recorded: parent `75f770`, no blockers.
- [x] Task-owned resource teardown recorded.

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


## 2026-09-19 — Acceptance resumed on bd01aca

Prerequisite GET /api/v1/profile is committed at `bd01acae2bc4e1b8a75bd95648e216535c790330`, parent review `c493` reports no issues. Fresh browser namespace `hashtag-browser-bd01aca`, then serial focused real Rails differential with separate fixture clones; maximum two full Rails setup attempts. No profile PATCH/app expansion permitted. Results below must distinguish executed gates from source/offline checks; independent parent harness review remains required and changes stay uncommitted.

### Executed result (supersedes the old blocked status)

- **Final fresh uninterrupted browser PASS:** header/history; real Follow/Unfollow and Feature/Unfeature clicks; reload persistence; representative home inclusion/removal; genuine profile featured-tag add/Delete; consolidated profile reload and public-profile presence/removal. Actual mutation ledger contains four header POSTs, collection POST, and DELETE `/api/v1/featured_tags/9204`, all 200. No API mutation substitutes or profile PATCH. Public-profile route viewed signed in.
- Three fresh resumed browser attempts: inherited 1-GiB web run interrupted requests (cause not proved); 2-GiB run reached a DOM-valued CDP wait; boolean-wait correction yielded final full pass. No application changes. Final web/container state running/non-OOM before cleanup.
- **Real Rails differential executed twice, final 54/59 strict matches; all 59 HTTP statuses match.** Pinned Rails child `696439e1…` (not latest), separate PG14 databases/media and restricted Rust roles. Methods, normalization, duplicate semantics, owner-ID/scope denials, nonzero featured count/date and ten-tag limit exercised with actual responses. Final nonzero count `"3"`, date `2026-09-19`, final collection count 10 match. No synthetic expected oracle.
- **Five unhidden validation-string differences keep strict differential FAIL:** invalid lookup 404 (`Not Found` vs `Record not found`); missing/empty name 400 (Rails adds `or invalid`); invalid name 422 (Rails lists three validation messages, Rust one); header over-limit 422 (Rails repeats the limit message three times, Rust once). No semantic status/count/ownership difference found in this matrix. Parent must decide ordinary observable wording requirements versus incidental upstream repetition; no app fix or extra attempt undertaken.
- First differential had four extra date differences induced by updating an older-ID status timestamp. Second/final fixture adds a new chronological highest-ID status instead; date/count differences disappear. Do not emulate ID-order/max-timestamp inconsistencies or treat this fixture correction as an application fix. Two-attempt cap reached.
- History raw values retained: first-day Rails 0/0 vs Rust 1/1 uses/accounts. Seven-day numeric-string shape checked; equality deliberately excludes Redis retention/activity versus current-DB aggregation. IDs paired bijectively; only undeclared equal-count collection tie order normalized. No peer workers/delivery; AddHashtag/RemoveHashtag remains deferred.
- Focused final offline checks **4 new + 8 existing remote-browser regressions passed**, shell/JS syntax and diff whitespace pass. `mise run harness-tests` attempted but fails in existing peer fixture test on macOS BSD `stat -c`; full aggregate not passed, no unrelated portability fix. Disabled-user probe is scope-confounded, not independent suspended-account precedence evidence.
- Source/client/helper hashes match; final generator reproduces executed scripts. Sanitized responses/clicks/screenshots, source/binary/image identities, restricted-role proof, checksums and cleanup under ignored `target/hashtag-acceptance-bd01aca/{browser,differential}`. NAS originals in `rustodon-hashtag-{browser,differential}-bd01aca-alice`, prior attempts separately retained.
- Task containers/PG volumes/networks/media/keys/env/session/build output removed and absence verified. No production, deployment, push or commit. Parent review of final harness remains pending; both issues remain open. Detailed normalization, resources, cases and mismatch table are in the harness README.
- Final independent review delegation was attempted and rejected by the session nesting limit (depth 1 of 1). No independent harness approval is claimed; hand off these uncommitted files to the parent reviewer.

## Closure — parent review 75f770

Closed/archived on parent approval of the bounded harness, no blockers. The actual strict differential remains **FAIL: 54/59**, with **all 59 statuses and equivalent rejection semantics**. The five error-text differences above are accepted as nonblocking ordinary compatibility unless an actual client dependency demonstrates otherwise; no bug-for-bug wording requirement, response normalization, synthetic pass, or changed raw evidence. Review approval is not a strict differential PASS.

Final real-click evidence proves the bounded **suggestion Add/Delete** path plus header controls, reload/public-profile state and representative home inclusion/removal. It does **not** prove typed-name entry. The captured empty editor displays the maximum-tags warning; [the focused metadata/typed-name follow-up](diagnose-featured-tag-limit-metadata-and-typed-name-editor.md) owns diagnosis and any minimum fix. The parent remains open for that actual remaining UI work. Peer hashtag delivery and Redis history semantics stay deferred; no full parent/peer/release completion claim.

This closure supersedes the historical pending-review/disposition notes without rewriting their evidence. No application change or extra fixture run. Final requested offline 4+8 and formatting/whitespace checks are recorded in the closure commit log/DEVLOG.
