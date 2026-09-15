# Audit everyday Mastodon frontend parity

## Summary

Sweep Rustodon for user-visible differences from Mastodon that appear during ordinary use of the bundled web frontend, prioritizing common workflows over obscure API compatibility edge cases.

## Requirements

- Exercise the basic integrated frontend journeys: sign-in/startup, home and public timelines, composing posts, media, replies, boosts, favourites, bookmarks, notifications, profiles, search, conversations, and common preferences.
- Compare frontend requests and behavior with the Mastodon 4.6.5 contracts already pinned in the repository.
- Prioritize missing, broken, stale, or materially different behavior that ordinary interactive users are likely to encounter.
- Exclude speculative edge cases and low-usage standalone API differences unless they directly break the bundled frontend.
- Record each actionable finding as a focused repository issue with reproduction evidence and expected behavior.

## Acceptance Criteria

- Representative everyday workflows are covered by browser checks, existing parity matrices, or focused source/contract inspection.
- Findings are ranked by user impact and confidence, with duplicates linked to existing open issues rather than recreated.
- Any new high-confidence gaps have focused issue files and open-index entries.
- The sweep records what was tested, what could not be exercised, and where coverage remains uncertain.

## Notes

- This is a discovery and triage sweep, not a mandate to fix every finding in one change set.
- Prefer a short list of reproducible everyday problems over a large list of theoretical differences.

## Sweep method

- Compared ordinary frontend request paths directly against the pinned Mastodon
  4.6.5 checkout at revision `1440d55b139e39ec722c2a3db7f60b66cd889048`.
- Reviewed the existing differential, browser-smoke, streaming, media, and
  acceptance coverage before classifying gaps, and linked existing issues rather
  than duplicating them.
- Exercised the deployed public frontend through landing, Live Feeds, local and
  remote profiles, profile media, status detail, remote image/video rendering,
  search, and sign-in pages. Captured screenshots and browser errors under
  `/tmp/rustodon-everyday-audit` without committing runtime artifacts.
- Used read-only aggregate production checks to validate visible counters. No
  post bodies, credentials, account relationships, or application rows were
  changed during this sweep.
- A reusable authenticated browser session was not available. Authenticated
  mutations were therefore classified only where pinned frontend calls and
  Rustodon route/write behavior make the result deterministic; existing
  Rails-differential and fixture coverage was used elsewhere.

## Ranked findings

### High impact

1. [Missing account statistics](self-heal-missing-account-statistics.md) make a
   healthy local profile show zero posts/follows despite visible activity, and
   undercount instance statuses. This was reproduced on the deployed frontend
   and confirmed by read-only database aggregates.
2. [Public, hashtag, and list streams](stream-public-hashtag-list-timelines.md)
   are silently unsupported, including the Live Feeds media/local/remote
   variants. Their bundled columns look connected but receive no live changes
   and have no periodic polling fallback.
3. [Poll creation, voting, and refresh](support-poll-voting-and-refresh.md) are
   unavailable even though the composer and rendered poll controls expose them.
4. [Quote-post creation](support-frontend-quote-posts.md) is omitted behind the
   prominent “Boost or quote” control. The frontend submits `quoted_status_id`,
   but Rustodon ignores it and publishes an otherwise-valid submission as a
   standalone post.
5. [Advertised media attachments](support-frontend-video-attachments.md) are
   selectable in the composer but HEIC/HEIF/AVIF, video, and audio uploads fail
   with 422 because creation and remote caching remain limited to four image
   formats. A live remote MP4 played only after showing a black poster because
   its `small` representation was the original MP4, not a still image.

### Moderate impact

6. [Status search](restore-frontend-status-search.md) always returns an empty
   `statuses` array, including the Mastodon-supported exact-URL resolution path.
7. [Instance activity metrics](compute-real-instance-activity-metrics.md) are
   hard-coded to zero, so the public frontend reports no active users while an
   eligible local user is visibly active.
8. [Hashtag and featured-tag controls](support-hashtag-and-featured-tag-controls.md)
   are unavailable: the missing hashtag lookup prevents its header controls
   from rendering, and the profile featured-tag mutations return 405.

## Lower-priority confirmed differences

- List create/edit/delete and membership controls are GET-only in Rustodon and
  return 405. This is already an explicit deferral in the read-only REST issue;
  it is real but below the everyday core selected for new focused issues.
- The status Embed modal calls an absent `/api/web/embeds/:id` route. Embedding
  is a visible menu action but was excluded from the prioritized set because it
  is much less common than timeline, compose, media, poll, and search workflows.
- Real Chromium coverage remains thin for compose/reply/edit/delete, social
  controls, rendered notifications, conversations, pagination, and frontend
  stream projection. Existing API/differential checks are strong, but successful
  HTTP contracts alone do not prove those UI journeys.

## Existing issues retained rather than duplicated

- V2 account search remains under
  [Restore v2 account search for clients](restore-v2-account-search.md).
- Historical followed-post convergence remains under
  [Diagnose missing posts from followed lain.com account](diagnose-missing-followed-lain-com-posts.md).
- The reported GIF case remains under
  [Diagnose missing remote animated media](diagnose-missing-remote-animated-media.md).
- Live settings confirmation remains under
  [Support frontend web settings API](support-frontend-web-settings-api.md);
  the original settings 404 is already implemented and fixture-proven.
