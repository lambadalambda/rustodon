# Diagnose featured-tag limit metadata and typed-name editor

## Summary

Focused remaining work for [Support hashtag and featured-tag controls](support-hashtag-and-featured-tag-controls.md). The accepted bounded browser controller proved suggestion Add/Delete, but not typed-name entry. Its captured empty editor says “You have reached the maximum number of featured hashtags.” Do not infer full editor compatibility from suggestion clicks.

## Requirements

- Diagnose the instance limit metadata → frontend state → featured-tag editor path on the actual source before proposing the minimum fix.
- Establish that an empty/below-limit editor exposes typed-name entry after normal load and reload; show the limit warning only at the actual ten-tag limit.
- Keep any eventual fix restricted to the relevant metadata/editor contract. No general profile PATCH, settings, schema/job or peer expansion without a scope checkpoint.
- Preserve the accepted local control and profile-read behavior; use real pinned-frontend input/clicks, not API substitutions, for browser mutation evidence.

## Acceptance Criteria

- A focused regression identifies the cause and proves any minimal correction.
- A fresh pinned-frontend run types a name (not merely a suggestion click), adds it, reloads, verifies the public-profile tag, deletes through UI, and verifies removal after reload.
- Below-limit and at-limit metadata/editor states agree with the supported local API limit. Record actual browser calls and sanitized evidence.
- Independent review and appropriately bounded verification before closure; no completion on source inspection alone.

## Evidence / scope boundary

- `bd01acae2bc4e1b8a75bd95648e216535c790330`, ignored `target/hashtag-acceptance-bd01aca/browser/evidence/profile-remove-reload.json`: empty editor renders the maximum-tags warning; `profile-add-reload.json` shows it with one tag too.
- Pinned Mastodon `1440d55b139e39ec722c2a3db7f60b66cd889048`, `features/account_edit/featured_tags.tsx`: `configuration.accounts.max_featured_tags ?? 0`; `canAddMoreTags = tags.length < maxTags` gates typed search and the warning.
- Rust source already contains `max_featured_tags: 10` in `src/mastodon/rest/serializer.rs`. Presence of those literals is not proof of the emitted nesting or the value reaching frontend state; do not assume the field is simply absent.
- Parent review `75f770` accepts the prior bounded harness without blockers, but explicitly leaves typed-name entry unestablished. This issue is created/indexed for diagnosis only: **no implementation or new browser execution in this closure turn**.
- Peer AddHashtag/RemoveHashtag delivery and Redis history/activity equivalence remain deferred, not part of this follow-up's completion claim.

## Focused follow-up (d17bec9)

- Verified the clean pinned checkout with `tools/mastodon-fixture verify-source`.
- Both REST instance serializers already emit `configuration.accounts.max_featured_tags = 10`; the frontend server model preserves the JSON unchanged.
- Pinned `features/ui/index.jsx` delays `fetchServer()` by 3000 ms. The previous empty-editor capture has no `/api/v2/instance` response. Test fully loaded state before inventing a metadata fix.
- Scope: focused typed-entry and ten-tag UI checks only, existing disposable browser adapter, no API mutation substitutions or full differential repeat. Parent review **293a7** approved the resulting code/harness without blockers.

### Result: completed on parent review 293a7

No application fix is necessary. Correct v1/v2 metadata was already present; the
old harness captured the pinned frontend's zero-fallback state before its delayed
(3000 ms) instance fetch fulfilled. New exact serialization and source-contract
regressions pass. Do not add a second limit or modify the pinned UI.

The new focused `typed` harness mode passed on unchanged d17bec9: empty normal
load/reload has typed entry and metadata 10; ten typed-name Add UI clicks yield ten
POST 200 responses, persisted editor/public-profile tags, and the warning only at
ten after readiness; ten Delete UI clicks yield DELETE 200, entry returns below
limit including reload, and final editor/public-profile reload confirms removal.
No API mutation substitutions. Existing accepted backend eleventh-tag rejection
remains the enforcement evidence, not an extra browser API call.

Exact run, bounds, hashes, attempts/limitations and cleanup are documented in
[`tools/hashtag-controls-browser/README.md`](../../tools/hashtag-controls-browser/README.md#typed-name-follow-up-on-d17bec9).
Sanitized JSON/DOM/screenshots and checksums are under ignored
`target/featured-typed-d17bec9/evidence/`. Five adapter + eight base harness tests,
one exact v1/v2 serializer regression, and one exact pinned source contract pass.
No differential/full-suite repeat or production access.

**Completed and archived** after independent parent review **293a7**, no blockers.
Combined with existing accepted evidence, the parent's remaining ordinary local
behavioral gap is satisfied. All ten persisted tags were checked in the public
profile response and DOM, not for simultaneous viewport visibility. Public routes
were viewed signed in; anonymous access is not established by this run. The warning
is correct only after readiness; the pinned upstream transient warning remains.
Strict Rails differential remains FAIL 54/59 with five previously accepted
nonblocking wording differences. Peer AddHashtag/RemoveHashtag and Redis retention
remain excluded; no new peer/history implementation or evidence is claimed.
