# Review account, media, and authentication release safety

## Summary

Complete the read-only security and correctness review of the account profile/settings,
media, browser authentication/session, password recovery, and mail changes against
the pinned Mastodon 4.6.5 behavior.

## Requirements

- Review authorization, CSRF, session/token revocation, settings defaults, media
  ownership, filesystem cleanup, media proxy access, reset tokens, and mail failure
  behavior.
- Compare relevant behavior with `/workspace/rustodon/target/mastodon-v4.6.5`.
- Run the available unit, integration, and guarded differential coverage.
- Record confirmed findings and unproved or deferred behavior without modifying the
  pinned reference checkout.

## Acceptance Criteria

- Final review reports severity-ordered findings, test coverage, residual gaps, and
  a release recommendation.
- No application files or pinned-reference files are changed by the review.

## Findings

### Resolved High: Locked-account status defaults can become public

The status writer now joins the account row when resolving an omitted visibility
and uses `private` for locked accounts without an explicit `default_privacy`,
matching `target/mastodon-v4.6.5/app/models/concerns/user/has_settings.rb`.
The guarded `account_settings` case creates an omitted-visibility status after
locking the account and verifies the resulting private audience.

### Resolved High: `default_quote_policy` is persisted but ignored by status creation

`POST /api/v1/statuses` now accepts `quote_approval_policy`; the write path
resolves the explicit request value or the stored account default and persists
the corresponding quote permission bits. Unit coverage verifies public,
followers, nobody, and private behavior, while the guarded `account_settings`
case verifies both a stored followers default and an explicit nobody request.
This matches the pinned controller behavior in
`target/mastodon-v4.6.5/app/controllers/api/v1/statuses_controller.rb`.

### Resolved Medium: Browser private-default settings do not normalize quote policy

The browser posting-defaults write now forces `default_quote_policy` to `nobody`
when `default_privacy` is `private`, matching the pinned
`posting_defaults_controller.rb`. The guarded settings case verifies this
normalization.

### Resolved High: Viewer-authorized media responses were publicly cacheable

Status-authorized local Paperclip responses now vary on `Authorization`,
`Cookie`, and `Signature`, retain public caching only for non-credentialed
requests, and use `private, no-store` for credentialed responses across full,
range, conditional, and `HEAD` responses. Cached remote media is also
private/no-store. ActivityPub status responses use credential-aware variation
and the same private/no-store rule for viewer-authenticated requests.

### Confirmed High: Most authenticated writes do not recheck account lifecycle

`write_account` only validates OAuth scopes and returns the resource-owner account ID
(`src/mastodon/write_repository.rs:10765-10776`). The stronger
`ensure_account_write_allowed_in` check locks the account and user rows and rejects
remote, suspended, disabled, unconfirmed, or unapproved accounts
(`src/mastodon/write_repository.rs:14887-14917`), but it is used only by the
account-profile, locked media, and status-creation paths.

Reports, conversation writes, bookmarks, mutes and pins, favourites, reblogs, status
updates/deletes, relationship writes, and notification writes still proceed from
`write_account` without that lifecycle recheck. A bearer authenticated before
`request_account_deletion` or administrative suspension can therefore reach those
writes after the account becomes unavailable. The existing
`stale_authenticated_account_writes_are_rejected_after_deletion_request` regression
only covers guarded media, profile, and status-creation paths. This diverges from
Mastodon's API-level `require_not_suspended!` behavior and is release-blocking for
writer-enabled deployment.

### Confirmed High: Self-service deletion retains local content for 30 days

The browser deletion route calls `request_account_deletion`
(`src/web.rs:9467-9516`). That method queues actor deletion immediately but schedules
`account_purge_job` at `created_at + 30 days`
(`src/mastodon/write_repository.rs:1621-1650,15428-15450`), leaving local statuses,
media, and most account content available in the retained account during the delay.

The pinned Mastodon self-service controller suspends the account and immediately queues
`AccountDeletionWorker` (`target/mastodon-v4.6.5/app/controllers/settings/deletes_controller.rb:39-42`);
that worker immediately calls `DeleteAccountService`, whose `purge_content!` runs before
the service returns (`target/mastodon-v4.6.5/app/workers/account_deletion_worker.rb:8-14`,
`target/mastodon-v4.6.5/app/services/delete_account_service.rb:91-95,146-160`). The
30-day scheduler is the separate deferred/admin path
(`target/mastodon-v4.6.5/app/workers/scheduler/suspended_user_cleanup_scheduler.rb:29-36`).
Rustodon therefore matches the delayed-purge issue's stated requirement, but not the
pinned self-service behavior. The product/API contract must explicitly choose one before
release; if Mastodon self-service compatibility is required, this is release-blocking.

### Confirmed Medium: Account status deletion leaves quote counters stale

`purge_account_statuses` deletes status rows directly without updating
`status_stats.quotes_count` (`src/mastodon/write_repository.rs:15914-15994`). The
Mastodon `Quote` model decrements the parent counter in `after_destroy_commit`
(`target/mastodon-v4.6.5/app/models/quote.rb:44-46,119-125`). The restored fixture
reproduction deleted source status `116845314048005201`: the target's quote rows and
counter changed from `1/1` to `0/1`. This causes incorrect REST counters after status
deletion or account purge.

### Confirmed Medium: In-flight actor deletion is not fenced by unsuspension

`cancel_activitypub_delivery` removes only undispatched deliveries and durable jobs
without a live lease (`src/mastodon/write_repository.rs:14045-14070`). An active
`deliver_activity` handler checks `suspended_at` once before the remote POST
(`src/worker.rs:1629-1641`) and does not recheck it after a concurrent unsuspension.
Therefore an already-running actor delete can still be sent after the account is
restored. `unsuspending_account_cancels_pending_deletion_jobs` covers only pending work,
not this in-flight boundary. Repeat deletion after reactivation also needs explicit
coverage because actor-delete delivery uses a stable per-actor/inbox logical key.

## Unproved and Deferred Behavior

- Hard power-loss compensation, live SMTP delivery/configuration, and
  adversarial remote-media fixtures remain unproved. Account purge now persists
  its cleanup paths in the durable job and performs filesystem removal after the
  database purge, with restored-fixture retry coverage.
- Full ActivityPub profile/update distribution, preview-card/link side effects,
  repeat actor-delete delivery after reactivation, and the remaining quote/federation
  workflows remain outside this review's proven surface.

## Coverage

- `mise run check`: formatting, strict Clippy, 194 library tests (194 passed,
  2 ignored), all non-ignored integration tests, cargo-deny, and pinned-fixture
  verification passed.
- `mise run differential`: all 19/19 Rails-versus-Rust cases passed against
  Mastodon 4.6.5 in 718.14 seconds, including account settings, browser
  authentication, password recovery/mail, local Paperclip authorization and
  deleted media, media writes, and OAuth.
- Account-settings cleanup restores account tags and account stats, and profile
  saves without avatar/header uploads preserve existing Paperclip files; the
  fresh aggregate now passes.
- `git diff --check` passed.
- The current restored-fixture gates pass `mise run worker-integration` with 40/40
  worker tests and `mise run mastodon-schema-integration` with 35/35 schema tests,
  including the account deletion, stale purge, and stale-authentication cases.
- An isolated PostgreSQL fixture run reproduced the quote-counter drift by deleting a
  status through the same raw SQL/cascade boundary used by purge: target quote rows
  `1 -> 0`, target `quotes_count` `1 -> 1`.

The differential account-settings coverage now proves successful avatar and
header uploads, including metadata, generated Paperclip files, and cleanup.
Browser/mobile recording, browser-form-specific evidence, and user-facing 2FA
management remain open. Live SMTP configuration, deployed shared-cache
behavior, hard power-loss database/filesystem compensation, and live peer
federation remain unproved.

## Follow-up: Lifecycle Fencing

- The authenticated-write lifecycle finding is now implemented across the
  identified local status, relationship, report, profile, media, conversation,
  notification, bookmark, mute, favourite, marker, and deletion entry points
  through `begin_account_write`. Profile/media filesystem work uses the same
  canonical account lock as lifecycle transitions.
- Actor-delete delivery now rechecks the account state while retaining the
  lifecycle lock through the final HTTP send, closing the unsuspend race.
- Account purge now decrements `status_stats.quotes_count` for accepted quotes
  whose source statuses are removed. The restored fixture reproduces and then
  covers the previously observed `1 -> 0` quote-row and counter correction.
- Restored worker coverage now passes 40/40, schema coverage passes 35/35, and
  `mise run check` passes with strict Clippy and dependency auditing.
- The stale-authentication regression now asserts `Unauthorized` across every
  identified authenticated write entry point after a deletion request.

## Recommendation

The initial media, browser, and authentication findings remain fixed and
regression-tested. The lifecycle implementation, quote-counter correction, and
in-flight actor-delete fence are now covered by targeted tests. External
acceptance evidence remains open, and the self-service deletion timing
divergence remains an explicit product decision:
Rustodon retains the required delayed-purge contract, while the pinned
Mastodon self-service path purges immediately. This review is not release
approval; keep the related issues open until their acceptance criteria are
proven.
