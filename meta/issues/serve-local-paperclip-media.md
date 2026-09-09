# Serve existing local Paperclip media

## Summary

Serve every existing database-referenced local media style from Mastodon's
filesystem layout without changing files or metadata.

## Requirements

- Support images, avatars, headers, emoji, previews, existing audio/video,
  `HEAD`, and required byte ranges under compatible public paths.
- Reject traversal, symlink escape, invalid styles, and unsafe path mappings.

## Acceptance Criteria

- Fixture media URLs and headers match Mastodon and all reads leave the tree and
  database unchanged.

## Completion

- Completed with shared database-backed path authorization, symlink-safe
  read-only filesystem access, compatible `GET`/`HEAD`/conditional/range
  responses, and Rails-versus-Rust differential coverage for every checked
  fixture file.
