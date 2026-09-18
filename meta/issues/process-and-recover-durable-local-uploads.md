# Process and recover durable local uploads

## Current status — complete (2026-09-18)

Independent worker review `2ea39` resolved the singleton-key high finding;
committed in `dee7526`. Recorded fault tests, committed replay retention and
successful retirement cover the lifecycle contract. Subsequent actual restricted
runtime/writer HTTP-worker evidence adds least-privilege integration. The HTTP
slice now retains terminal processing=3 rows; the deleted-row description below
is historical, not current behavior.

Parent explicitly approves closure and archival. This status supersedes earlier
“open”, “uncommitted”, “review pending” and closure-proposal instructions below;
those record historical handoff stages, not outstanding work. No gates were rerun
for this docs-only reconciliation. Actual transport-loss/power-loss simulation,
full fixture/release matrices and remote-media implementation remain unclaimed.
The [frontend rich-media parent](support-frontend-video-attachments.md) is now
**complete** on combined reviewed local and remote evidence after `fe6e461`.

## Summary

Worker/filesystem/lifecycle-only slice of [local rich-media uploads](support-local-rich-media-uploads.md), following reviewed persistence at `949d503`. HTTP v2 remains non-live.

## Requirements

- Register processing on an existing queue lane with `ResourceClass::Media`.
- Private root-confined raw input, verified size/hash, bounded processor; no public Paperclip raw route.
- Generation/claim/account fences, without holding the account lock during processing; commit exact final manifest before output writes.
- Preserve focus/description and committed output on retries, cancellation and ambiguous commit; reload before destructive reconciliation.
- Raw-only successful retirement; bounded exact-owner recovery for incomplete staging, orphans, deletion and exhausted retries. Legacy cleanup must spare owned staging.
- No new migration, privileges, queue lane or public schema without a scope checkpoint.

## Acceptance Criteria

- Deterministic repository/worker lifecycle fault regressions and real tiny image/video/audio success run in bounded disposable NAS resources.
- Independent parent review after implementation; changes left uncommitted.
- Exact evidence and follow-up API contract recorded; no claim of HTTP/browser completion.

## Notes

- Read persistence review boundaries before implementation. Stop after two substantive review/fix rounds.

## Implementation and evidence (2026-09-18)

- Both processing and recovery register on Maintenance with `ResourceClass::Media`.
  Processing intents use the persistence generation key and four attempts. The
  existing maintenance scheduler enqueues recovery only when its kind is idle.
- Raw bytes live under mode-0700 `.local-upload-input` beneath the configured media
  root, with the existing root-confined/no-symlink filesystem implementation.
  The database raw manifest is relative to this private root, not the public
  Paperclip route. Reads check exact length and SHA-256 before processing.
- Claim/read and install/cleanup hold the canonical account lock; bounded codec
  processing does not. Installation rechecks account usability and claim/generation,
  commits and reloads the immutable output manifest before touching output files,
  and preserves current focus/description. Ambiguous publication never triggers
  output deletion or rewriting on ready replay.
- `retire_ready_in` validates the public manifest and removes only raw ownership
  after durable raw unlink. Published files remain untouched. Orphan cleanup
  validates exact paths, retains ownership on unlink failure, and forgets only
  absent public rows. Legacy rollback-create jobs spare durable staging owners.
- Recovery uses keyset pages of 100, durable continuations, a one-hour grace for
  incomplete staging and exhausted/lost accepted work, and separate runtime-role
  job inspection. Undispatched or live jobs are retained. Failed owners do not
  prevent page continuations; a later scheduled scan retries their cleanup.
- NAS evidence: `/srv/workspaces/rustodon-upload-worker-949d503-alice/evidence/`.
  `run.sh` records the bounded native tools runner; `red.log` proves the missing
  retirement primitive before implementation. Final results: `worker-final.log`
  6/6, `repository-final.log` 4/4, `legacy-worker-final.log` 1/1, and
  `clippy-final.log` focused strict Clippy pass. See DEVLOG for exact selectors.
- Worker coverage includes real image/video/audio through `WorkerExecutor`,
  partial output failure, pre-/post-publication commit fault replay, raw unlink
  failure, size/hash mismatch, symlink rejection, stale generation/claim,
  concurrent edits/deletion/account disabling, cancellation during processing,
  durable retry exhaustion, legacy staging protection, and 100-row scan bounds
  with continuation despite one failed owner.
- Tests used the supplied immutable media image, PostgreSQL 14.23, and a restored
  task-owned database. Lifecycle worker tests used the fixture owner connection;
  the separate restricted-role persistence test passed. This is not a claim of
  end-to-end least-privilege worker, full worker/media, browser, or peer gates.
- No migration, public schema, privilege, HTTP-route, production, or macOS
  compatibility changes. Keep open for independent parent review; uncommitted.

## Follow-up HTTP v2 contract (not implemented here)

- Authenticate and recheck account usability under the account lock; commit
  staging before raw bytes, durably write/verify private input, then commit
  acceptance with `processing_job(identity)` before returning pending. Reconcile
  ambiguous staging/acceptance by reloading the exact identity, never speculative
  unlink. Do not send raw paths to clients or expose them through `/system`.
- Define compatible pending/ready/error polling and authenticated pending
  focus/description edits and deletion. Publication already preserves latest
  edits; deletion must retain ownership for recovery and fence processing.
- **Terminal abandonment currently deletes the pending media row**, as in the
  reviewed persistence primitive. Agree on the observable polling/error contract
  before exposing v2; do not pretend durable failure details are stored. Any new
  persistent failure state requiring a migration needs its own scope checkpoint.
- Prove focused HTTP acceptance/poll/edit/delete and composer preview/playback,
  attach/post/reload before enabling the API or closing the parent issue.

## Review round 1 — scheduler singleton key

- Reviewer `ea39b9de` found one high-severity defect: a second root recovery tick
  failed because the existing singleton job had no logical key. No other high
  findings; compactness approved.
- Extracted the actual tick enqueue path into `schedule_recovery` and gave its
  root spec the stable `local-upload-recovery:root` key. Cursor continuation keys
  are unchanged. No terminal-failure or HTTP scope expansion.
- TDD regression calls that same scheduling path twice without completing root,
  asserts `Existing` ownership of the same job and exactly one live root. NAS red
  reproduced `singleton jobs require a logical key`; green passes all seven
  local-upload worker tests and the existing local-media cleanup regression.
- Focused strict Clippy, local formatting and diff checks pass. Logs are in the
  existing NAS evidence directory as `review1-{red,green,legacy,clippy}.log`.
  Changes remain uncommitted for parent review.
