# Agent Guidance

## Build and Test Host

- Run all builds, tests, formatting, lint, and container workloads on
  `lain@secunda.local`, not on the local coding machine.
- Use isolated workspaces beneath `/home/lain/rustodon-parity/`. Do not overwrite
  `/home/lain/repos/rustodon` or its untracked `tracked-configs/` directory.
- Use rootless Podman and task-specific resources. Never synchronize instance
  environment files, credentials, backups, or unrelated untracked files.
- See `docs/testing-on-secunda.md` for workspace and reference-source setup.

## Mastodon Reference Source

- Use the existing pinned Mastodon 4.6.5 checkout at
  `/workspace/rustodon/target/mastodon-v4.6.5` when inspecting upstream code,
  tests, migrations, routes, serializers, policies, or behavior.
- On Secunda, the existing checkout is
  `/home/lain/repos/rustodon/target/mastodon-v4.6.5`. Use it read-only when the
  canonical `/workspace/` path is absent; do not fetch a replacement merely
  because the source is absent on the local coding machine.
- The expected revision is `1440d55b139e39ec722c2a3db7f60b66cd889048`.
- Do not clone, fetch, pull, or otherwise retrieve Mastodon source from GitHub
  when this checkout is present.
- Treat the checkout as read-only. Do not modify it.
- Include this local checkout path explicitly in every Mastodon-related
  subagent prompt.
