# Support hashtag and featured-tag controls

## Summary

The bundled frontend exposes a hashtag header with Follow/Unfollow and Feature/Unfeature actions, plus add/remove controls for profile featured hashtags. Rustodon lacks the hashtag lookup needed to render the header and provides only the featured-tag collection reads, so these ordinary controls are absent or return 405 and do not persist.

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

## Evidence

- Mastodon 4.6.5's bundled hashtag page first calls `GET /api/v1/tags/:tag`; only a successful response renders the header and its follow/feature controls. The profile UI separately uses the featured-tag collection create/delete routes.
- Rustodon has none of the tag lookup, follow/unfollow, or feature/unfeature routes. Featured-tag collection routes are GET-only, so the hashtag controls are absent and profile mutations receive 405 responses.
- The missing tag lookup was explicitly deferred by the completed empty-read compatibility issue; this issue owns it because it is a prerequisite for the bundled controls.

## Bounded implementation

- [Local API/database slice](implement-local-hashtag-control-apis.md) owns the first implementation; this parent remains open for browser evidence and remaining compatibility work.
- ActivityPub AddHashtag/RemoveHashtag peer projections require a separate protocol scope and are deferred, not supplied by the local controls. Historical stream cleanup remains separately tracked.

### Local slice status

Local API/database implementation and focused restricted-role HTTP/stream tests are present, pending parent independent review; see the subissue for exact executed gates. Header history has a bounded current-public-status aggregate, not Mastodon Redis retention parity. Browser controls, Rails HTTP differential coverage, and peer feature projections remain unproved/deferred. Do not close this parent on local evidence alone.
