# Port mixed profile-media preservation matrix

## Summary

Extend existing credentials tests with one mixed GIF-avatar/JPEG-header request, text-only preservation, one-slot replacement/removal, and atomic rejection. Verify exact pinned 4.6.5 expectations before adopting discovery cases; retain supported raw-GIF behavior.

## Acceptance Criteria

Assert API results, independently decoded stored files/URLs, unchanged opposite slot, and no partial database/file mutation on rejection. Use a permanent existing fixture gate; no new formats or transcoding.

## Notes

- Subissue of [selected matrix ports](port-mastodon-media-and-browser-matrices.md).
- Tests first; separate topical implementation and independent review.
- Pending; no implementation or execution evidence claimed.
