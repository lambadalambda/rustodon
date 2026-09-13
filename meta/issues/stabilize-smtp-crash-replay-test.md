# Bound and stabilize SMTP crash-before-ack replay coverage

## Summary

The final combined NAS worker gate stalled at
`smtp_acceptance_before_job_ack_is_retried_with_the_same_message_id` beyond the
two-hour parent job budget. Earlier100-worker runs passed. The worker was still
running remotely after SSH termination; parent explicitly terminated only the
identified worker test process, allowing fixture cleanup and offline harnesses
to finish. No live services or unrelated resources were touched.

## Requirements

- Diagnose the exact synchronization/lease path; do not assume a production bug.
- Preserve accepted-message identity and real retry/ack assertions.
- Make fixture waits bounded and deterministic where practical, without extending
  production SMTP/queue deadlines or weakening fences.
- Bound NAS worker commands inside the workload, not solely the SSH client.
- NAS-only verification, independent review, topical commit.

## Acceptance Criteria

- Focused crash/replay and combined100-worker gate pass with bounded shutdown.

## Evidence

- `logs/final-extended-workers.log` under NAS audit-green: hung at the named test;
  final SIGTERM evidence is interruption, not a behavioral red assertion.
- Task container exited0 after the parent terminated testPID27628; the worker
  fixture postgres27186 was removed and final offline harnesses passed.

## Tests-only fixture correction (awaiting parent verification)

Source inspection identifies two unsafe fixture paths, not the proven exact cause
of the historical combined-run hang: the 100ms lease can expire before legacy
mail identity is durably initialized (before SMTP), while the test waits for an
acceptance notification instead of observing the completed worker; and the SMTP
fixture's DATA loop ignores zero-byte reads, so peer EOF can spin indefinitely.
Earlier 100-worker runs passed; the later >2h interruption is not a deterministic
red assertion or proof that either path caused that particular stall.

The owned worktree prepares an ordinary no-DB EOF regression snapshot first:
SMTP handshake through 354, partial DATA and write-half EOF must yield
UnexpectedEof. The parent must run the pre-fix snapshot under a process watchdog:
the buggy loop may prevent a cooperative Tokio timeout/abort from progressing.
No red or green run was performed by this editing task.

The separate correction patch rejects DATA EOF, removes the redundant acceptance
Notify, awaits the exact injected durable-job completion failure directly, uses
30s live test leases, and explicitly expires only the selected job owned by
smtp-before-ack (asserting exactly one row changed). It preserves legacy identity
creation/persistence, the changed SMTP domain, duplicate identity, and distinct
message assertions. Both replayed and distinct job rows must be durably removed
before waiting for final server completion. The entire scenario has a 20s test
budget; processing futures are inline, and fixture tasks are aborted and joined
on error, panic, or timeout. The EOF regression has a 5s cooperative budget.

No production deadlines/fences change. Build/test/fmt/NAS/SSH execution, the
outer NAS workload watchdog, and commits remain parent-owned. Keep this issue
open until the focused and combined NAS gates have been verified.


## Parent execution evidence

- New ordinary EOF regression failed pre-fix with its5-second `Elapsed` error
  (`smtp-eof-red.log`,exit101), rather than requiring the outer10-second kill.
- Corrected EOF regression passes immediately (`smtp-eof-green.log`). The bounded
  combined worker gate passes100/100 (`smtp-combined-green.log`,454.81s).
- All-feature debug/release, strict Clippy and offline harnesses pass on the
  correction. Default compilation exposed a misplaced inherited `test-support`
  cfg from insertion of the new test; the parent restored that cfg on the old
  SMTP scenario, with its fresh default-profile check now running.
- Independent reviews approved the EOF and synchronization changes. These reds
  prove the fixture defect, not the exact cause of the historical two-hour stall.

## Completion

Restoring the original scenario's `test-support` cfg makes the full default
all-target test run pass (`smtp-default-cfg-green.log`). Together with EOF
RED/GREEN, bounded combined100-worker pass, all-feature debug/release, strict
Clippy and offline harness greens, the focused issue is complete. No production
mail, queue, timeout or fencing behavior changed. The watchdog recipe is recorded
in the NAS runbook; future SSH termination is not treated as remote cleanup proof.
