# Support hashtag and featured-tag controls

## Summary

The bundled frontend exposes a hashtag header with Follow/Unfollow and Feature/Unfeature actions, plus add/remove controls for profile featured hashtags. The original implementation lacked hashtag lookup and collection writes. Ordinary local controls are now complete on the accepted API/stream, browser and Rails evidence below.

## Requirements

- Implement `GET /api/v1/tags/:tag` so the bundled hashtag header and its controls can render.
- Implement `POST /api/v1/tags/:tag/follow`, `/unfollow`, `/feature`, and `/unfeature` with Mastodon-compatible Hashtag responses.
- Implement `POST /api/v1/featured_tags` and `DELETE /api/v1/featured_tags/:id` for the signed-in account.
- Enforce account ownership, tag normalization, duplicate, limit, and lifecycle rules.
- Keep home-timeline inclusion and live-stream fanout consistent with followed-tag state.

## Acceptance Criteria

- A hashtag page renders its header, history, and current follow/feature state.
- Follow, Unfollow, Feature, and Unfeature controls on a hashtag page update immediately, persist after reload, and affect the home timeline/profile as Mastodon 4.6.5 does.
- A user can also add and remove featured hashtags through the bundled profile UI and see the public profile update.
- Invalid, duplicate, unauthorized, suspended, and over-limit cases match Mastodon behavior.
- Differential and browser tests cover each visible control.

## Original gap (historical)

- Mastodon 4.6.5's bundled hashtag page first calls `GET /api/v1/tags/:tag`; only a successful response renders the header and its follow/feature controls. The profile UI separately uses the featured-tag collection create/delete routes.
- Rustodon has none of the tag lookup, follow/unfollow, or feature/unfeature routes. Featured-tag collection routes are GET-only, so the hashtag controls are absent and profile mutations receive 405 responses.
- The missing tag lookup was explicitly deferred by the completed empty-read compatibility issue; this issue owns it because it is a prerequisite for the bundled controls.

## Bounded implementation

- [Local API/database slice](implement-local-hashtag-control-apis.md) is archived on source review round 2 and its executed restricted-role API/stream gates; the typed-name editor follow-up below completes the parent.
- ActivityPub AddHashtag/RemoveHashtag peer projections require a separate protocol scope and are deferred, not supplied by the local controls. Historical stream cleanup remains separately tracked.

### Local slice status

Local API/database implementation and focused restricted-role HTTP/stream tests are present; parent source review round 2 approved `8e22f52`. See the local subissue for exact executed gates. Header history has a bounded current-public-status aggregate, not Mastodon Redis retention parity. Do not close this parent on local evidence alone.

- [Bounded browser and Rails differential acceptance](accept-local-hashtag-controls-browser-differential.md) is archived on parent review **75f770**, no blockers. Reviewed/archived [profile-read prerequisite](support-authenticated-profile-read.md) `bd01aca` (review `c493`) resolves the old 404. Final uninterrupted browser controller passed header controls, genuine **suggestion Add/Delete**, reload/public-profile state and representative home inclusion/removal. No API mutation substitutions or profile PATCH; typed-name entry was not established.
- Actual pinned Rails strict differential remains **FAIL: 54/59**, with **all 59 statuses and equivalent rejection semantics**. Parent accepts five error-string differences as nonblocking ordinary compatibility pending an actual client dependency (invalid lookup; missing/empty/invalid name; repeated header-limit wording). No normalization or fabricated PASS; raw evidence preserved. API child closure makes no browser claim.
- The [typed-name editor follow-up](diagnose-featured-tag-limit-metadata-and-typed-name-editor.md) is now complete on independent parent review **293a7**, no blockers; see the result below.
- Final focused offline checks passed 4+8; full aggregate remains blocked by existing macOS `stat -c` incompatibility. History DB-versus-Redis values are retained without semantic-parity claims. Peer AddHashtag/RemoveHashtag projections remain explicitly deferred, not completed by these closures. See accepted child for exact source, attempts, evidence and cleanup.

### Completed ordinary local acceptance — parent review 293a7

The focused diagnosis found **no missing instance metadata**: the previous harness
snapshot preceded the pinned frontend's 3000-ms delayed instance fetch. A fresh
bounded browser run on unchanged d17bec9 now proves typed-name Add, persistence,
public-profile response/DOM persistence of all ten tags and the at-limit warning
after readiness (not simultaneous viewport visibility),
Delete, return of input below limit, and final removal after reload. See the linked
[diagnosis issue](diagnose-featured-tag-limit-metadata-and-typed-name-editor.md) and
its exact evidence. No arbitrary replacement limit or application/UI patch.

Together with the already accepted API/stream, header/home browser and Rails
comparison evidence, this satisfies the remaining ordinary-workflow behavioral
gap. **Completed and archived** after independent code/harness review **293a7**,
no blockers. Public routes were checked signed in, not anonymously. The upstream
transient maximum warning before readiness remains; no application source changed.
The strict Rails result stays **FAIL 54/59**, with all 59 statuses matching and five
previously accepted nonblocking wording differences. No full-suite or differential
repeat. [Peer AddHashtag/RemoveHashtag](support-featured-tag-peer-add-remove.md) and
Redis retention equivalence remain excluded, not satisfied by this local closure.
