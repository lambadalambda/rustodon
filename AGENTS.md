# Agent Guidance

## Mastodon Reference Source

- Use the existing pinned Mastodon 4.6.5 checkout at
  `/workspace/rustodon/target/mastodon-v4.6.5` when inspecting upstream code,
  tests, migrations, routes, serializers, policies, or behavior.
- The expected revision is `1440d55b139e39ec722c2a3db7f60b66cd889048`.
- Do not clone, fetch, pull, or otherwise retrieve Mastodon source from GitHub
  when this checkout is present.
- Treat the checkout as read-only. Do not modify it.
- Include this local checkout path explicitly in every Mastodon-related
  subagent prompt.
