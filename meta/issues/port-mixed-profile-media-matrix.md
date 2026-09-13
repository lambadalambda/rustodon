# Port mixed profile-media preservation matrix

## Summary

Permanent credentials HTTP coverage for mixed GIF-avatar/JPEG-header upload, text-only preservation, both slot replacements/removals, and rejected multipart atomicity. Retain supported animated GIF output; no new formats, production performance rewrite, or widened HTTP deadlines.

## Acceptance Criteria

Assert API/reload results, independently decoded stored/served bytes, unchanged opposite slot, and no partial account/user/file mutation on rejection. Wire the matrix into selected and all-case fixture execution.

## Completion Evidence — Parent NAS Reports

- Exact mixed matrix initially passed with `profile=test,opt-level=3`, total **83.36s**, unchanged fixtures/assertions/deadlines.
- Permanent wiring offline regression: **RED before / GREEN after**. **All offline harnesses PASS.** This verifies all-case dispatch wiring, not a claim that the complete live all-case fixture lane ran.
- Actual-media sensitivity control: temporarily made an omitted header remove existing media. **FAIL in 14.84s**, specifically at text-only preserved-header `Slot` equality; initial mixed upload completed in **3516ms**, within unchanged budgets. This was a preservation failure, not a setup/timeout substitute for red.
- Mutation reverted; permanently wired optimized matrix **PASS**, log **`mixed-profile-wired-green.log`**. Final duration has not yet been read/reported; do not reuse the earlier 83.36s value for this run. Parent confirms **no production mutations remain**.
- Parent preserved the early `$ROOT/tools/verify-worker-media` preflight from `410fb8f`. The offline sandbox supplies an executable verifier stub and asserts exactly one call before Cargo/provisioning. No pinned-source/fixture oracle was removed.
- All execution above is parent-reported NAS evidence. This worktree agent ran no builds/tests/formatting/NAS/SSH workloads and made no commits. Parent is awaiting SSH-agent unlock before committing; no workaround attempted.

## Permanent Implementation

- `tests/differential/mixed_profile_media.rs`: one unique disposable fixture account; mixed upload; text-only preservation; both replacements/removals with populated opposite slots; invalid-avatar/valid-header and inverse rejection; overlong-note rejection with new paths and already-installed paths. Check API reload, full account/user equality on rejection without secret-row dumps, media-root path/hash/mtime snapshots, decoded disk/HTTP bytes, GIF frames/delays/static PNG, and superseded URL/file removal.
- `tools/mastodon-fixture`: subshell-scoped opt-level 3 for mixed discovery/execution only; allowlisted/writable selector; all-case mode checks presence, skips mixed in the ordinary batch, and runs it once optimized after existing comparisons. Other selectors/CLI builds retain their profile. Parent owns the Rust module/test registrations and merged harness preflight.
- `tools/tests/mixed-profile-harness-test.py`: real shell control flow with external services stubbed; selected/all-case optimization and isolation, failure propagation, verifier ordering, and `--fixture-script PATH` for before/after evidence. Automatically included by `tools/check-harnesses`.
- Temporary `tests/mixed_profile_media_prepare.rs` was removed from permanent changes after obtaining the measurements below.
- Independent read-only reviews found the HTTP matrix, scoped execution policy, and shell/offline wiring sound. Earlier username-alphabet and populated-opposite-slot review findings were corrected. Optional child-process environment and explicit existing-comparison-order assertions remain deferred.

## Exact Fixture / Timing Findings

- Vendored `avatar.gif`: **85,810 bytes, 128×128, 10 frames** (not the initially reported 142), delays 5/7 centiseconds; SHA-256 `d2562762b0ff220aafd128e6b266fa3f172eb4a92c0dcb584e0b28e8618d7d2e`. Matches the pinned worker manifest. The smaller pinned `attachment.gif` is single-frame and was **not** substituted.
- Parent isolated debug preparation: GIF inspect **25ms**, prepare **12072ms**, validate **226ms**, output 10 frames at 400×400; JPEG inspect **74ms**, prepare **109ms**, validate **81ms**. Exact GIF preparation alone exceeded the existing deadline. The selected HTTP test uses current-thread Tokio with an in-process server.
- Permanent execution optimizes the test profile, retaining debug assertions/overflow checks. Shared budgets remain **2s connect / 10s total / 5s read**. No retry, frame truncation, global profile change, or production optimization was introduced.

## Pinned Oracle and Ownership

- Subissue of [selected matrix ports](port-mastodon-media-and-browser-matrices.md).
- Read-only 4.6.5 oracle: `/Users/lainsoykaf/repos/rustodon/.local-instance/audit-reference/remaining/spec/requests/api/v1/accounts/credentials_spec.rb`, extracted from existing exact cached image `696439e1ada71d0cf3d51d4d6a4744d6e40b57aafa64980b18f4d3b78230d0cf`. Lines 53–67 and 99–129 establish mixed GIF/JPEG success; lines 83–96 establish overlong-note 422. Preservation/removal/atomicity extend that request spec rather than being falsely attributed to it.
- Pinned revision `1440d55b139e39ec722c2a3db7f60b66cd889048`; fixture provenance in `tests/fixtures/README.md` and `fixtures/worker-media/mastodon-v4.6.5/{README.md,SHA256SUMS}`. Canonical `/workspace/rustodon/target/mastodon-v4.6.5` and Secunda `/home/lain/repos/rustodon/target/mastodon-v4.6.5` absent locally; no fetching or 4.7 substitution.
- Owned worktree `/Users/lainsoykaf/repos/ilar-task-remaining-profile` (`task/remaining-profile`). Parent owns execution, integration, issue-index closure, and commits.

## Final Diagnostic-Only Follow-up

The mutant exposed oversized panic output from derived `StoredImage::Debug`. Replaced it with format, byte count, SHA-256, frame count, and first-frame dimensions; derived `PartialEq/Eq` still compare every encoded byte and decoded-frame field. Added the ordinary regression `stored_image_debug_is_compact_and_equality_remains_exact` before the implementation edit. No execution of this final diagnostic-only change is claimed. Independent read-only review approved the bounded output, unchanged exact equality, regression coverage, and source-level compile plausibility; no blockers found. Core matrix/wiring completion evidence above predates this change. Parent may finalize closure after integrating/reviewing this small diagnostic delta and its normal checks.

## Completion

Permanent exact-selector execution passes (`mixed-profile-wired-green.log`,88.68s)
with unchanged request budgets. The omitted-header mutation fails at the intended
preservation assertion, not timeout. Offline wrapper RED/GREEN and all harnesses
pass. Compact-debug regression was independently RED then GREEN on NAS; strict
all-target/all-feature Clippy passes after formatting. Reviews approved matrix,
profile scoping, and concise diagnostics without weakening exact byte/frame equality.
The full all-case wiring is covered offline; this is not a claim every unrelated
full differential case was executed successfully.
