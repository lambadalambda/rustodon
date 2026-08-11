# Build the Rails-versus-Rust differential test harness

## Summary

Create a compatibility harness that runs equivalent behavior against Mastodon
4.6.5 and Rustodon and reports meaningful wire- and data-contract differences.

## Requirements

- Start Mastodon and Rustodon against independent clones of the same fixture
  database and media tree.
- Send identical HTTP requests to both implementations.
- Compare response status, relevant headers, and canonicalized JSON.
- Normalize only explicitly documented nondeterminism such as request IDs,
  generated timestamps, and random test tokens.
- Provide a database diff mode for later write tests.
- Reserve comparison hooks for emitted ActivityPub documents, durable jobs, and
  media-tree changes.
- Produce focused mismatch output that identifies the JSON path, header, table,
  or file that differs.
- Make individual compatibility cases runnable without executing the entire
  suite.

## Acceptance Criteria

- A sample instance endpoint case passes against Mastodon 4.6.5 and a fixture
  Rust response.
- An intentional JSON, status-code, and header mismatch each produce a failing
  test with readable diagnostics.
- Nondeterministic normalization is covered by tests and centrally declared.
- Harness setup never points at a non-test database or media root.
- The README documents how to run one differential case and the complete
  compatibility suite.

## Notes

- Depends on `bootstrap-rust-workspace.md` and
  `pin-mastodon-4-6-5-fixtures.md`.
- This harness should compare observable behavior, not SQL ordering, callback
  counts, Redis keys, or Sidekiq payload representation.
