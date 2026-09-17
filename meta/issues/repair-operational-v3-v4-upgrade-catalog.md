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
- The rehearsed inverse migration plus prior writer ACL restore makes the prior
  image's preflight pass against the same clone.
- Formatting, focused normal tests, and an independent blocker/high review pass.

## Evidence

- The disposable physical-volume rehearsal failed with catalog fingerprint
  `9df60a97c09a0dc108c13c25ecedafd74089381e3ae01fb039beef0d030cc31f`,
  `runtime_role=Some("rustodon_runtime")`, `writer_role=None`,
  `expected_owner=Some("rustodon_migrator")`, and 221 catalog entries.
- Production remains on operational schema v3 and application revision
  `986e5c950c37e90f9ff296a11e5302c4e558daf4`; no production migration was run.
- Root cause: the registered
  `f2c4c137fd98a64e61a3a3795c4d9fe11c79b974357a5fabe14a4cb3e10dae6e`
  was generated from a newer PostgreSQL catalog. Version-sensitive
  `attstattarget`/null serialization, NOT NULL constraints, and `MAINTAIN`
  privileges make it inapplicable to the repository's pinned PostgreSQL 14
  deployment boundary.
- The expected v4 fingerprint now registers only the exact PostgreSQL 14.23
  physical-clone result above. The catalog query, detailed ACL/security checks,
  migration DDL, and supported major remain unchanged. Ordinary unit coverage
  pins that provenance and asserts column statistics remain fingerprinted.
- Production and heavy container fixtures were not touched while applying this
  source-only correction; disposable-clone migration/preflight and rollback
  rehearsal remain deployment-gate evidence rather than local test claims.
- TDD captured the old `f2c4…` value as the expected red failure, then passed
  after the single registered hash changed. Focused provenance/statistics tests
  passed, the full library suite passed (**320 passed, 5 ignored**), and format
  plus diff checks passed using the pinned toolchain. Native tests used the
  temporary macOS Paperclip compatibility patch, restored byte-for-byte and
  verified against pre-run hashes. Independent review found no blocker or high
  finding.
