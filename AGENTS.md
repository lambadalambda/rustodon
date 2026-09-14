# Agent Guidance

## Build and test safety

- Use the task and lane definitions in `mise.toml` and
  [`docs/testing.md`](docs/testing.md).
- Run container-backed, browser, and peer fixtures only in disposable,
  non-production environments with task-owned databases, media roots, ports,
  identities, and container resources.
- Run heavy fixture lanes sequentially with explicit CPU, memory, process, and
  wall-time bounds. Clean only resources created by the current task; never
  broadly prune a shared container engine.
- Synchronize only intended tracked source and explicit new fixture files. Never
  copy instance environments, credentials, backups, `.git`, build output, or
  unrelated untracked files into test workspaces.
- Preserve production SSRF, TLS, signature, privilege, and test-only feature
  boundaries. A synthetic or image-only check is not a substitute for its named
  source, database, browser, or peer gate.

## Mastodon reference source

- The compatibility target is Mastodon 4.6.5 revision
  `1440d55b139e39ec722c2a3db7f60b66cd889048`.
- `mise run fixture-obtain` places the verified source under the repository's
  ignored `target/mastodon-v4.6.5` path.
- Treat the reference checkout as read-only. Do not modify it, use a dirty or
  wrong-revision tree, or substitute another Mastodon version.
- Reuse an existing verified checkout when available instead of fetching a
  replacement. The source-contract lane verifies the revision before use.
