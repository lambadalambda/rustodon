# Repair operational schema v3 to v4 deployment upgrade

## Summary

The exact Rustodon production PostgreSQL 14 volume was cloned into disposable
volumes before deploying revision `87aa8fe`. Running the candidate's
`admin migrate-operational-schema` against that clone applies the v4 migration
inside its transaction but rejects the resulting catalog fingerprint as unknown.
The deployment is blocked before production migration or cutover.

## Requirements

- Identify why an exact supported production v3 catalog upgrades to a catalog
  hash different from the registered v4 fingerprint.
- Make the v3 to v4 upgrade portable across the supported PostgreSQL version
  without weakening catalog-drift, ownership, role, or privilege validation.
- Add a focused red/green regression that starts from the v3 catalog and proves
  migration plus current validation.
- Preserve the exact rollback boundary; do not mutate production while
  diagnosing or testing the fix.

## Acceptance Criteria

- The candidate migration and preflight pass against a disposable clone of the
  exact production volume.
- The production cutover stages a bounded rollback that restores operational
  schema v3 and the prior writer ACL contract before restarting the prior image.
  Per the project's prototype-scale policy, do not repeat the earlier full
  restore/rollback rehearsal solely for additional ceremony.
- Formatting, focused normal tests, and an independent blocker/high review pass.

## Evidence

- The first disposable physical-volume rehearsal failed with catalog fingerprint
  `9df60a97c09a0dc108c13c25ecedafd74089381e3ae01fb039beef0d030cc31f`,
  `runtime_role=Some("rustodon_runtime")`, `writer_role=None`,
  `expected_owner=Some("rustodon_migrator")`, and 221 catalog entries.
- Root cause had two layers. The original
  `f2c4c137fd98a64e61a3a3795c4d9fe11c79b974357a5fabe14a4cb3e10dae6e`
  fingerprint came from a newer PostgreSQL catalog whose null serialization,
  NOT NULL constraints, and `MAINTAIN` privileges differ from PostgreSQL 14.
  The intermediate `9df60a…` fingerprint also retained the already-provisioned
  writer ACLs because the dedicated migrator environment intentionally has no
  writer credentials and therefore did not identify that role for normalization.
- PostgreSQL 14.23 now pins the single role-normalized v4 fingerprint
  `328d793a7d32a38bd0e3c86e196877c93b18e3a84793d67eb2069e2288e30ac7`.
  The migration command accepts an explicit writer role name, with no writer
  credentials, and applies it transaction-locally before catalog validation.
  Runtime preflight and migration therefore validate the same exact catalog;
  separate writer diagnostics continue validating the full least-privilege ACL
  contract. Unknown catalog drift is not normalized or accepted.
- TDD first pinned the PostgreSQL 14 fingerprint and then the credential-free
  migration CLI. The full library suite passed (**320 passed, 5 ignored**), an
  all-target/all-feature check passed, formatting and diff checks passed, and
  the temporary macOS Paperclip compatibility patch was restored byte-for-byte
  to SHA-256 `d4464a229b751669d6e273e5757ea388d9b958eb1549a917b8d2c60c818acbaf`.
  Independent review found no blocker or high finding.
- The final ARM64 candidate migrated a disposable clone of the exact production
  PostgreSQL 14 volume from v3 to v4, reapplied the writer contract, and passed
  complete preflight with writable cloned media and disposable Redis. Only the
  known SMTP-disabled and unverified persisted-tag-domain warnings remained.
- Revision `cc0c5fb6123ec4c1d33a937df54bb2ad9e9a7657` was then deployed to
  `rustodon.social`. The cutover staged the prior units, prior writer-grant SQL,
  and an inverse v4-to-v3 transaction before mutation. Production migration,
  writer grants, and preflight passed; web and worker started on the candidate;
  operational schema reports v4; worker readiness reports `ready=true`; and
  public `/health`, `/ready`, and `/api/v2/instance` return successfully.
