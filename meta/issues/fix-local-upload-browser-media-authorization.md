# Fix local-upload browser media authorization

## Summary

Direct blocker subissue of [Verify local rich uploads in the pinned browser](verify-local-rich-upload-browser.md) and [Support local rich-media uploads](support-local-rich-media-uploads.md), on `373da56`. Implements the narrow plan approved by independent design review `2156dc`.

## Requirements

- Paperclip GET/HEAD only: functional authenticated owners may read ready local unattached originals/previews/ranges. Pending, failed, absent, remote unattached and raw staging remain denied, even with stale bytes.
- Attached media keeps existing status visibility, including authorized followers and report-manager deleted exception; deleted/dangling attachment status IDs never become unattached. Preserve historical attached readiness conventions.
- Only absent Authorization permits browser-session resolution. Internally authenticate the backing token using existing READ_STATUSES and require_user, verify identities; never expose/mint tokens, touch sessions, set cookies or relax CSRF. Explicit bearer errors/scope/ownership never fall back.
- Authorization precedes metadata, conditional and range responses. Preserve exact metadata path checks and openat2 confinement.
- Recognized media denials/errors (even anonymous) and authenticated successes use private/no-store and Vary Authorization, Cookie, Signature. Anonymous public attached success caching unchanged.
- Production code limited to web and repository; no schema, grants, jobs, global auth refactor, CORS, signature expansion or capability URLs. Stop before scope expansion.

## Acceptance Criteria

- TDD using persisted HTTP sessions/tokens with restricted runtime/writer roles; extend existing local_upload_http support rather than a second codec harness.
- Cover owner bearer/cookie GET/HEAD/ranges/previews; anonymous/other owner/pending/failed; attached private owner/follower/unrelated; deleted/dangling; expired/revoked/logout/nonfunctional sessions; bearer precedence; no session timestamps or Set-Cookie; conditional/range denial ordering.
- Preserve existing pure web media/cache/range tests. Record exact red/green commands and results with bounded task-owned PG14 and supplied NAS runtime; no production or browser execution.
- Leave uncommitted for independent review and subsequent parent browser rerun. Keep issue open pending that evidence.

## Notes

Loaded repo-issues and nas-podman skills before implementation. No new authentication scope or permission model.

## Implementation and evidence

- Implemented in web + repository only; focused persisted HTTP matrix **1/1**,
  extended real-codec HTTP lifecycle **1/1**, pure web tests **91 passed / 2 ignored**,
  focused strict Clippy and fmt/diff checks pass. Two baseline RED failures prove
  unattached bearer and private attached cookie blockers independently.
- See DEVLOG entry and `target/media-viewer-evidence/` for exact commands, source
  hashes, bounded NAS resource names and retained red/green/setup logs.
- Existing unsatisfiable-range behavior is Rack/Rails' final **404**, not 416;
  preserved and now covered by private denial caching. No range contract expansion.
- Uncommitted; independent parent review and isolated browser rerun remain pending.
  Nested review delegation was unavailable at this task depth. Keep this issue open.

## Review1 follow-up

No blocker/high finding. Resolve two directly relevant compact mediums before
Review2: (M1) private/no-store + media Vary on recognized denials/errors even
anonymous, preserving anonymous public success caching; (M2) retain prior optional
bearer semantics for attached media outside limited federation, while new unattached
grants require functional users and explicit Authorization never permits cookie
fallback. Add persisted HTTP regressions and rerun focused gates. No rearchitecture
or scope expansion; remain uncommitted and pause after Review2.

M1/M2 implemented and verified: separate REDs for missing anonymous denial cache
headers and overly strict public-attached empty bearer; final focused HTTP **1/1**,
real-codec lifecycle **1/1**, pure web **91 passed / 2 ignored**, strict Clippy and
fmt/diff checks green. Follow-up production diff only touches `src/web.rs`, with
no repository/schema/grant/shared-auth change. Exact commands, bounded NAS cleanup
and hashes are in the Review1 DEVLOG entry and `target/media-viewer-evidence/`.
Paused for Review2; remain uncommitted.
