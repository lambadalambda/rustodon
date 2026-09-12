# Agent Guidance

## Build and Test Host

- Run builds, tests, formatting, lint, and container workloads on the authorized
  NAS worker `podman-worker`, not the local coding machine or unstable Secunda.
- Parent exclusively owns sequential NAS SSH/Podman execution; editing subagents
  must not start independent NAS workloads. Use disposable task workspaces below
  `/srv/workspaces/` and explicit rootful socket routing; never change the user's
  default Podman connection or prune unrelated resources.
- Sync tracked source and explicitly selected test files only, never instance
  environments, credentials, backups, or unrelated untracked files.
- See `docs/testing-on-nas.md`. Secunda instructions remain historical/fallback;
  preserve `/home/lain/repos/rustodon` and its `tracked-configs/` untouched.
- If the SSH agent cannot sign, retry once sequentially, then request an unlock.

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
