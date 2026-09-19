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
