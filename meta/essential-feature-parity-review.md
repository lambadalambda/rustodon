# Essential feature parity review

Reviewed baseline: `1861d33` (`main`), after the live Pleroma PEM fix.

## Verdict

The essential feature surface is broadly implemented, but **essential behavioral parity is not established**. There are security defects and ordinary federation/client failures inside that implemented surface, not just missing acceptance recordings.

The repository already distinguishes implementation from acceptance: `docs/v1-acceptance-matrix.md` leaves ordinary browser/mobile publishing (`ACCEPT-04`) and bidirectional Mastodon peer convergence (`ACCEPT-06`) open. The successful Pleroma Create proves one interoperability path, not the lifecycle or privacy matrix.

Priorities below use **P1** for release-blocking security/core correctness and **P2** for other actionable compatibility/reliability defects. They are review priorities, not CVSS ratings.

## Method and verification limits

- Independent read-only reviews covered inbound federation, outbound federation, authenticated social REST, and browser/OAuth authentication. Separate secondary reviews assessed clean-checkout gates and durable queue behavior. The primary review cross-checked critical source paths and consolidated the duplicated follow-options finding.
- This is a targeted source review, not an exhaustive audit or a new full release-gate run. Except for the two parser reproductions explicitly identified below, the scenarios are **source-traced regression specifications, not executed end-to-end results**.
- The prescribed repository-external Mastodon 4.6.5 checkout at revision `1440d55b139e39ec722c2a3db7f60b66cd889048` and project-local `target/mastodon-v4.6.5` were unavailable. No upstream source was fetched. No new Rails differential comparison is claimed.
- No live application writes, credential inspection, or federation replays were performed. No production code was changed; red/green regression tests belong in the individual fixes.

### Checks performed

- `tools/mastodon-fixture verify`: passed (static checksums and pinned metadata).
- `tools/tests/rustodon-browser-smoke-test`: passed (shell/mock tests of the smoke script, **not** a live browser session).
- A temporary native Rust crate imported the **unmodified** `src/mastodon/activitypub_inbox.rs` and ran five assertions through `parse_activity`: Create with omitted summary accepted; `summary: null` rejected; empty-string summary accepted; actor Update without an image accepted; the same Update with a standard `Image.url` icon rejected. `cargo run --offline --quiet` passed all five assertions of the existing behavior. This reproduces R04 and R06 at the real parser boundary, without compiling the Linux-only application or writing application data.
- Full application Rust/Clippy, restored PostgreSQL integration, live browser/mobile, and live-peer matrices were not rerun. The host compiler is Rust 1.97.0 versus required 1.97.1, and the application relies on Linux filesystem APIs. R16 is an additional independent clean-checkout prerequisite defect.
- Documentation whitespace and local Markdown links were checked before commit.

## Release-blocking findings

### R01 — P1: fetched wrappers can fabricate posts under another remote author

**Source:** `src/worker.rs:2779–2817`, `2890–2947`, `3039–3112`; `src/mastodon/write_repository.rs:3688–3708`.

Announce target fetching authenticates the requested origin, but a fetched Create only has to match that requested URI in its outer `id`. Its claimed actor and embedded Note can consistently name an unrelated victim origin. The writer binds the Note to the claimed actor, not to the origin that supplied it. A known victim actor is accepted from the local account cache without author-signature verification or a fetch of the alleged Note.

**Regression scenario:** a locally followed attacker announces `https://evil.example/create/1`. That endpoint returns a Create with the matching outer ID, `actor: https://victim.example/users/alice`, and an embedded public Note whose `id` is a new URI under the victim origin and whose `attributedTo` is Alice. Attacker content can be inserted under Alice's account. The nested-Announce embedded-Note path has the same trust defect (`worker.rs:2864–2870`, `3081–3089`). This concerns new forged URIs; existing-object ownership checks do not make it safe.

**Fix/proof:** bind embedded-object authority to authenticated provenance, or dereference the claimed canonical object on its authoritative origin before materializing it. Reject or safely refetch adversarial Create and nested Announce wrappers; assert no forged status, mention, notification, or boost is committed. Do not conflate outer activity URI equality with proof of embedded-object authorship.

### R02 — P1: bookmarks/favourites bypass revoked private-post access

**Source:** `src/mastodon/repository.rs:1437–1443`; `src/mastodon/rest/loader.rs:1354–1360`, `3101–3109`, `3191–3196`; `src/mastodon/write_repository.rs:7723–7755`.

Saved-status selection checks association ownership and deletion, then uses `preauthorized_statuses` without current root visibility checks. Removing a follower leaves their bookmark/favourite associations intact.

**Regression scenario:** Bob saves Alice's followers-only post; Alice removes Bob as a follower and edits the post to contain **new private content**. Bob is denied by status-show but still receives the edited content through bookmarks/favourites. This is ongoing access to new content, not merely a retained old copy.

**Fix/proof:** apply current viewer authorization before exposing saved statuses, retaining correct association cursors. Test removal and subsequent edits through both saved endpoints, not just status-show.

### R03 — P1: password recovery does not fence in-flight authentication

**Source:** `src/mastodon/write_repository.rs:8662–8686`, `8864–8869`, `9017–9041`, `9347–9442`; `src/web.rs:11026–11051`, `10338–10355`.

Login commits password verification before creating the session in another transaction. Password recovery revokes sessions/tokens that already exist, but session creation accepts only the earlier authenticated user ID. A login verified against the old password can therefore create a fresh session after recovery commits. The security-page password-change path also verifies the current password outside its write transaction, then invokes an administrative reset operation that does not revalidate that credential.

**Regression scenarios:** pause login after successful credential verification, recover the account, then resume session creation. Separately pause a password-change request after its current-password check, recover the account, then resume: the stale request can overwrite the newly recovered password.

**Fix/proof:** atomically couple authentication and its authorized write, or compare a credential/session generation inside the write transaction. Barrier-controlled tests must prove recovery fences both schedules. Adding a lock without checking the earlier authentication version is insufficient.

### R04 — P1: ordinary no-CW Notes are rejected after HTTP acceptance

**Source:** `src/mastodon/activitypub_inbox.rs:453–458`; `src/mastodon/activitypub.rs:922–926`; `src/worker.rs:4517–4520`; `src/web.rs:4325–4360`.

The parser rejects a present `summary` unless it is a string. Rustodon's own serializer emits `summary: null` for an empty content warning. A correctly signed ordinary Create can receive HTTP 202, then be permanently failed by the worker without creating a status. Update and fetched targets use the same validator.

**Executed proof:** the unmodified parser accepts an otherwise identical Create with omitted or empty-string summary and rejects `summary: null`.

**Fix/proof:** handle nullable optional Note fields consistently. Add serializer-to-parser round trips and worker Create/Update/fetched-target tests; assert remote state, not just inbox HTTP success.

### R05 — P1: initial actor resolution loses the followers collection

**Source:** `src/remote.rs:1034–1047`, `1782–1795`; `src/mastodon/write_repository.rs:3113–3133`, `3162–3180`, `11842–11864`, `2847–2900`; `src/worker.rs:4658–4671`.

`RemoteActor` and its initial upsert omit `followers_url`, whose schema default is empty. Private Note classification requires that stored collection URI. Consequently a freshly resolved account's followers-only Note is classified as limited rather than private; shared-inbox relevance then requires an explicit local actor recipient and ignores the established follow.

**Regression scenario:** resolve a previously unknown actor, complete Follow/Accept, then deliver a Note addressed only to its followers collection through `/inbox` (use a string summary to isolate R04). The Note is dropped despite the local follow. A later actor Update can populate the field, and pre-populated restored fixtures mask the defect.

**Fix/proof:** preserve the advertised followers collection through resolution and persistence. Exercise the complete discovery → persistence → follow → shared-inbox private-post path, without seeding the field directly.

### R07 — P1: changing options on an accepted follow creates another request

**Source:** `src/mastodon/write_repository.rs:6608–6670`, `6674–6693`, `6741–6787`, `7396–7423`; `src/web.rs:14919–14948`; unique actor-pair index in `fixtures/mastodon/v4.6.5/database.sql:9035`.

For remote/locked targets or a silenced source, the follow writer checks only existing requests, not accepted follows. Setting notify/reblogs/languages creates another request instead of modifying the live relationship. This affects locked local accounts as well as remote ones. For a remote target, accepting the second Follow tries to insert another actor-pair follow because its new URI differs from the existing follow, violating uniqueness. Unfollow can also leave the spurious pending request behind.

**Regression scenario:** Follow → Accept → change options → inspect relationship and options → remote Accept (where applicable) → unfollow. Existing immediate duplicate-Follow tests do not cover this sequence.

**Fix/proof:** update an accepted relationship before choosing the request branch; retain its URI, create no new Follow/request notification for preference changes, and prove local-locked and remote lifecycles.

### R08 — P1: cancelling an intermediate delivery breaks its ordering chain

**Source:** `src/mastodon/write_repository.rs:5439–5446`, `14815–14824`; `src/jobs.rs:469–479`, `1127–1144`, `1194–1206`.

Cancellation physically deletes unleased jobs, while an ordered job only checks its immediate predecessor and the ordering marker can still point to the deleted job. This can disconnect later work from an earlier live delivery in the same source-account/inbox stream.

**Regression schedule:** Like A is live; Like B is queued behind A. Unlike B deletes B; dispatch Undo B with missing B as predecessor, allowing it to run while A is live. Unlike A queues Undo A behind Undo B. After Undo B completes, Undo A can overtake still-live A. Remote final state now depends on handling Undo-before-Like, contrary to the promised wire-order fence.

**Fix/proof:** preserve ordered cancellation no-ops, repair chains atomically, or fence all relevant earlier outstanding work. Add the multi-job cancellation schedule across two workers; an immediate Follow/Undo pair is insufficient.

## Other concrete compatibility findings

### R06 — P2: normal avatar/header objects reject entire actor Updates

**Source:** `src/mastodon/activitypub_inbox.rs:403–406`, `743–763`, `802–806`; `src/mastodon/write_repository.rs:3265–3268`, `10355–10374`; outgoing shape at `src/mastodon/activitypub.rs:610–627`.

The actor validator treats `icon`/`image` as URI objects using `id`/`href`, but ordinary actor images contain `{"type":"Image","url":"https://…/avatar.png"}`. The entire Update, including display name and bio, fails permanently after inbox acceptance. The writer also lacks the required `url` extraction, so fixing validation alone is insufficient.

**Executed proof:** the unmodified parser accepts a Person Update without an icon and rejects the same document with a standard Image/url icon. Add full serialized-actor Update tests covering profile fields and persisted image URLs.

### R09 — P2: private boosts can omit their followers audience

**Source:** `src/worker.rs:567–572`, `5083–5092`; `src/mastodon/write_repository.rs:9267–9274`, `13898`, `14071–14080`, `14153–14156`; schema default at `fixtures/mastodon/v4.6.5/database.sql:448`.

Normal local-account creation leaves the stored `followers_url` empty. Unlike Note serialization, Announce builders use it directly, giving followers-only boosts `to: []` and only the original author in `cc`. Sending that payload to follower/shared inboxes does not supply the missing protocol audience.

**Fix/proof:** derive the canonical local followers URI consistently in writer and worker. Test a newly created account with an empty stored field, private boosts, and a real shared-inbox recipient. The malformed wire audience is source-confirmed; peer-specific handling was not run.

### R10 — P2: distinct same-second Updates reuse an activity ID

**Source:** `src/worker.rs:760–764`; `src/mastodon/activitypub.rs:671–678`; `src/web.rs:4352–4353`, `4363–4377`; `src/jobs.rs:414–426`; `src/worker.rs:2140–2150`.

Status and actor Update IDs use whole seconds, although internal versioning tracks microseconds. Deliver one version, then edit and deliver another within the same second: both bodies share an activity ID. A Rustodon receiver treats this as an identity/body conflict, returns 409, and the sender classifies that response as permanent. Stale-payload coalescing does not help if the first version already arrived.

**Fix/proof:** distinct durable wire IDs for distinct versions; stable identity/body on retries. Test two sequentially delivered same-second versions. The confirmed receiver contract is Rustodon's, not a freshly tested Mastodon duplicate policy.

### R11 — P2: fully qualified local mentions silently lose recipients

**Source:** `src/mastodon/write_repository.rs:13758–13779`, `13431–13452`, `5925–5926`, `5965–5970`.

The mention parser retains the domain in `@bob@local.example`; resolution compares it with `accounts.domain`, which is NULL for local accounts, without normalizing the configured local domain. A direct post succeeds but lacks Bob's mention/access grant and notification.

**Fix/proof:** canonicalize local handles during resolution. Compare `@bob` and the fully qualified local handle through create and edit, checking mention rows, notifications, and Bob's ability to read the direct post.

### R12 — P2: followed-hashtag home posts lack user-stream events

**Source:** `src/mastodon/repository.rs:1333–1350`; `src/mastodon/write_repository.rs:14623–14641`, `14705–14714`, `5979–5985`.

REST home includes public posts selected by preserved `tag_follows`; stream recipient selection covers author/account followers but not hashtag-only followers. Those clients miss creates and subsequent edits/deletions despite the post belonging to their home timeline.

**Fix/proof:** align eligible stream recipients with supported home selection while retaining visibility/policy checks. Use a tag follower who does not follow the author; verify REST and stream lifecycle agree.

### R13 — P2: security-page password challenges bypass abuse limits

**Source:** `src/web.rs:691–720`, `10292–10349`, `9839–9874`, `10080–10122`; `src/mastodon/write_repository.rs:8924–8937`, `8971–8982`.

Unlike login, current-password checks for password changes and 2FA management reach bcrypt without the shared guessing limit. A valid/stolen browser session and CSRF token expose an unthrottled password oracle; failed guesses do not consume the login attempt budget.

**Fix/proof:** a shared per-user/IP reauthentication budget across sensitive settings routes, before bcrypt. Test repeated incorrect guesses and switching routes; verify legitimate reauthentication still works. This requires an existing session, not an unauthenticated attacker.

### R14 — P2: supported public OAuth clients cannot revoke tokens

**Source:** `src/web.rs:8958–8972`, `9138–9153`, `11463–11475`; `src/mastodon/write_repository.rs:8593–8625`, `9498–9514`, `12622–12638`.

Public clients can exchange S256-bound authorization codes without a secret, but revocation requires a nonempty secret and the repository rejects non-confidential applications unconditionally. These authorization-code tokens have no expiry, so merely deleting a client's local copy does not invalidate a leaked token.

**Fix/proof:** permit public-client identification for revocation with exact token/application ownership, retaining secret authentication for confidential clients. Test authorize → exchange → revoke → rejected bearer for a public application.

### R15 — P2: OAuth discovery advertises unsupported response modes

**Source:** `src/web.rs:9116–9119`, `9313–9329`, `9346–9371`.

Discovery advertises query, fragment, and form_post. Authorization drops/ignores `response_mode` and always returns query parameters, including for denial. Clients selecting form_post or fragment from discovery receive the wrong callback behavior; form_post also fails its expectation that codes stay out of callback URLs.

**Fix/proof:** minimally advertise only query and reject unsupported modes, or preserve and implement all advertised modes. Exercise both consent and denial.

### R16 — P2: test/Clippy gates cannot compile a fresh checkout

**Source:** `src/remote.rs:2520`, `3232`, `3265`; `.gitignore:1`; `mise.toml:16–22`, `28–38`, `88–90`; `.github/workflows/ci.yml:11–27`, `29–47`.

Ordinary tests use compile-time `include_str!` against the ignored Mastodon source checkout. The CI check job runs all-target/all-feature tests and Clippy without obtaining it. Static fixture verification does not fetch source; acquisition in a separate CI job does not populate the check job's filesystem. Even default tests hit two unconditional test-module inclusions.

**Fix/proof:** preferably use a checked-in, explicitly test-only signing key and shared helper for ordinary transport unit tests; keep upstream source requirements in contract tests. Alternatively make source acquisition a prerequisite of both test and lint, not a parallel sibling of check. Verify in an empty-target clean checkout. This finding was independently cross-checked; no full CI build was executed here.

## Operational observations: scope separately before fixing

These are lower-priority source-supported concerns, not measured production incidents. They should not enlarge the security/federation fixes.

### R17 — P2: dispatcher repeatedly filters retained stream history

**Source:** `src/jobs.rs:754–770`, `1310–1311`; `migrations/rustodon/0001_operational.sql:67–72`; `src/worker.rs:5778`, `5798–5799`.

Stream events remain `dispatched_at IS NULL` and therefore stay in the pending outbox index. Dispatch excludes their kind in the query, but the pending index cannot isolate non-stream events. Idle polling must filter the retained history to establish there is no dispatchable work; LIMIT bounds results, not historical rows examined. Default polling is every 250 ms. Latency and practical capacity impact require EXPLAIN/measurement.

First investigate an appropriate dispatch access path and query predicate. **Do not simply delete the outbox:** dispatched rows preserve `record_outbox_once_in` deduplication, and stream cleanup needs a defined horizon and lagging-cursor policy.

### R18 — P2: permit waiters consume all worker execution slots

**Source:** `src/worker.rs:363–403`, `5747–5757`; `src/jobs.rs:468–480`; defaults at `src/config.rs:591–600`, `640–658`.

All default worker loops claim across the same lanes before obtaining resource permits. With five execution slots and four HTTP permits, five older slow independent deliveries can occupy all slots, including one permit waiter; a later core notification waits despite not requiring HTTP. Media has the analogous problem. Lease renewal keeps waiters alive. This is bounded head-of-line blocking, not a permanent deadlock; live timing was not measured.

If required for small-instance responsiveness, reserve core capacity or make claims resource-aware. Test slow remote work plus a ready core job, rather than only proving the remote concurrency ceiling.

## Why current tests missed ordinary failures

1. **Fixtures skip discovery/persistence boundaries.** Pre-populated followers URLs hide R05 and R09.
2. **Input/output contracts are tested separately.** Outgoing `summary: null` and Image/url objects are valid serializer outputs but missing from successful inbox fixtures (R04/R06).
3. **Short lifecycle tests miss transitions.** Immediate duplicate Follow/Undo cases do not cover accepted-follow preference changes or cancelled intermediate ordering jobs (R07/R08).
4. **Endpoint-specific authorization tests miss alternate reads.** Status-show authorization does not establish saved-collection authorization (R02).
5. **HTTP success is not peer-state convergence.** Queuing an activity can return success before parsing permanently fails; the Pleroma investigation exposed the same distinction at another boundary.

No large rewrite is needed as a prerequisite. The immediate maintainability improvement is to put identity, audience, authorization, and version decisions behind shared tested boundaries, with cross-boundary lifecycle tests around them. Keep fixes small and topical rather than reorganizing the large router/writer files during security repair.

## Recommended sequence and tracking

1. Security: [object provenance](issues/fix-fetched-activitypub-object-provenance.md), [saved-status authorization](issues/restore-saved-status-authorization.md), [reauthentication/recovery](issues/fence-browser-reauthentication-and-recovery.md).
2. Interoperability: [ordinary inbound shapes and audiences](issues/repair-activitypub-ingestion-and-audiences.md), [relationship and delivery lifecycle](issues/repair-relationship-and-delivery-lifecycles.md).
3. [Client workflow gaps](issues/close-reviewed-client-workflow-gaps.md) and [clean-checkout gates](issues/restore-clean-checkout-quality-gates.md).
4. Run a real peer/client lifecycle matrix from newly discovered accounts: public/unlisted/private/direct posts, mentions, follows/options, profile/status edits, likes, boosts, Undo, deletion, and access revocation. Assert received state, identities, visibility, and notifications—not just responses.
5. Decide separately whether to schedule R17/R18 performance work; record baseline measurements first.

## Existing acceptance gaps (not new defects)

- Pinned Mastodon 4.6.5 signed actor GET fails with HTTP 503 during signer-key resolution; tracked in `issues/prove-mastodon-peer-federation-compatibility.md`.
- Pleroma ingestion of the original mention is proven after `884032b`, but recipient notification visibility and the complete bidirectional lifecycle are not.
- Recorded browser/mobile publishing, production cutover/rollback, real disk exhaustion, sustained end-to-end load, and hard-power-loss evidence remain open.
