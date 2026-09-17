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

## Prototype operating mode

- Rustodon is in the prototype phase. Optimize for learning, shipping ordinary
  Mastodon-compatible workflows, and keeping the code understandable—not for
  exhaustive production ceremony or hypothetical scale.
- `rustodon.social` is the single project-owned experimental instance. Treat its
  data with care, but do not apply multi-tenant production standards to every
  reversible application deployment; brief planned downtime is acceptable.
- Use safeguards proportional to the actual risk. Routine, reversible app-only
  changes normally need focused tests, an immutable candidate, a retained
  rollback image/container, and health/readiness checks—not a full fixture
  matrix, disaster-recovery rehearsal, or exhaustive edge-case proof.
- Require stronger backup, restore, and rollback work for destructive data
  changes, irreversible migrations, identity/credential changes, or operations
  with a credible risk of losing the only instance's state. Do not perform such
  work merely for procedural completeness when the change is additive and
  recoverable.
- Heavy database, browser, differential, cutover, and peer lanes are milestone
  evidence. Run them when requested, before a release claim, or when the changed
  behavior specifically needs them; do not make every prototype fix wait for all
  lanes.

## Mastodon reference source

- The compatibility target is Mastodon 4.6.5 revision
  `1440d55b139e39ec722c2a3db7f60b66cd889048`.
- `mise run fixture-obtain` places the verified source under the repository's
  ignored `target/mastodon-v4.6.5` path.
- Treat the reference checkout as read-only. Do not modify it, use a dirty or
  wrong-revision tree, or substitute another Mastodon version.
- Reuse an existing verified checkout when available instead of fetching a
  replacement. The source-contract lane verifies the revision before use.

## Compatibility and scope management

- Target observable Mastodon compatibility for ordinary user workflows, APIs,
  wire protocols, persistence invariants, and peer interoperability. Bug-for-bug
  or implementation-level parity is not required.
- Do not reproduce known Mastodon races, security or privacy defects, or
  incidental quirks unless a real client or peer demonstrably depends on that
  behavior. Prefer safer correctness while preserving required external
  contracts.
- Before a focused fix expands into a new protocol, durable job type,
  schema/privilege boundary, or changes across more than three subsystems, stop
  for a scope checkpoint. State the minimum fix, proposed expansion, exclusions,
  and verification level; split follow-up work where practical.
- Pause and report after two substantive review/fix rounds. Fix directly relevant
  blockers and high-severity defects; normally record scope-expanding medium or
  unrelated findings as follow-up issues instead of growing the current change.
- Treat deferred fixture execution as an evidence boundary, not an invitation to
  expand implementation or harness scope indefinitely. Clearly distinguish
  executable test source from gates actually run on the current tree.
