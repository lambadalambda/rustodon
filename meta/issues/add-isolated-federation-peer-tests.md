# Add isolated bidirectional federation peer tests

## Summary

Add repeatable Mastodon/Rustodon and Pleroma/Rustodon interoperability scenarios on Secunda, complementing—not replacing—the fixture differential suite.

## Requirements

- Start distinct disposable instances with independent databases, signing identities, media, and active federation workers.
- Reuse the existing pinned Mastodon 4.6.5 source/images and fixture lifecycle where practical. Pin Pleroma source/image and dependencies explicitly before claiming its coverage.
- Keep any private-network/test-CA transport capability behind explicit test-only build/runtime boundaries. Never weaken ordinary release SSRF, TLS, signature, origin, redirect, or response-size checks.
- Use fresh cross-peer actors without preseeded actor caches or follows; assert discovery, accepted follows, and push-ingested public statuses in both directions first, then extend lifecycle/privacy coverage.
- A resolver/search fetch must not substitute for successful push ingestion of a status. Check actual remote state rather than treating HTTP 2xx as completion.
- Run all builds, tests, and containers on `lain@secunda.local`, with bounded task-owned resources and cleanup; never touch the live tunnel, lain.com, or instance secrets.

## Acceptance Criteria

- A documented command starts isolated peers on Secunda and exercises real application GET/signature/inbox/worker paths.
- Each claimed direction/activity passes assertions on actor/object identity and received state; unsupported or blocked scenarios fail or remain explicitly open.
- Test-network routing fails closed for unconfigured destinations and is unavailable to ordinary release builds, with regression tests.
- Cleanup removes only the run's recorded resources; repeated runs do not require production credentials or public origins.

## Notes

- Subissue of [Secunda parity verification](run-essential-parity-gates-on-secunda.md).
- Existing differential mode compares two implementations under one logical fixture identity and does not run Mastodon Sidekiq; it cannot serve as the peer convergence test unchanged.
- Begin with one thin Mastodon smoke and share the scenario runner with a Pleroma bootstrap rather than building a general orchestration framework.
