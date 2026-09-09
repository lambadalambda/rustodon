# Implement account profile updates

## Summary

Implement supported credential, profile, and preference changes.

## Requirements

- Add `update_credentials` for text, fields, posting/profile preferences,
  avatar/header update, and image removal.
- Preserve settings JSON, profile formats, URLs, media metadata, and
  NULL-versus-empty behavior.

## Acceptance Criteria

- Permission and differential rollback tests match Mastodon for every supported
  field without exposing the full Rails settings surface.

## Notes

- Implemented the text, fields, posting/profile settings, bot/visibility flags,
  attribution-domain normalization, profile hashtag refresh, and `rel="me"`
  field formatting slice.
- Owner-role schema and Rails differential coverage pass for the supported slice;
  local avatar/header upload and removal now have Paperclip processing, metadata,
  route, and unit coverage. Guarded HTTP differential coverage now also proves
  the media request/response and filesystem rollback. ActivityPub actor
  distribution, profile-link verification, and preview-card reattribution
  workers remain deferred to their owning federation issues. The written
  acceptance criteria for this issue are satisfied and the issue is archived.
