# Verify the Issue 26 and Issue 27 SQLite Schema Release

- **Status:** planned
- **Owner issue:** [#73](https://github.com/FabienGreard/TouchGrassBar/issues/73)
- **Implementation:** [PR #65](https://github.com/FabienGreard/TouchGrassBar/pull/65)
  and [commit bbf56f18](https://github.com/FabienGreard/TouchGrassBar/commit/bbf56f18e8a74f6dd0e9ccaffdb071871d6d9ea3)

## Scope

This tracker covers the first release that contains the issue 26 SQLite
schemas and the issue 27 retained-history update. It covers the database
coordinator, sanitized read model, Codex usage index, Claude usage index, and
private usage synchronization ledger. It includes the Codex usage-index change
from version 6 to version 7. Version 7 adds the UTC day to each private Fast
turn reference. This change permits exact 30-day private-detail cleanup while
sanitized daily token aggregates stay for 60 days. It also includes the
sanitized read-model change from version 6 to version 7. This version adds a
dedicated durable generation-one Profile backfill completion field without
changing the stored activation time.

The release fixture rehearsal uses synthetic data only. It does not use a
cloud deployment or raw provider content.

## Execution plan

1. Generate the candidate release fixture with the current module versions.
2. Rehearse the Codex and sanitized read-model version 6 to version 7
   migrations on disposable copies.
3. Open each immutable stable fixture through the database coordinator.
4. Verify the upgraded module versions, object catalog, strict tables, foreign
   keys, and value invariants.
5. Verify that the Codex aggregate window is 60 days and that its model, cost,
   and Fast turn-reference window is 30 days.
6. Open each upgraded fixture a second time and verify byte and backup
   idempotence.
7. Run the release compatibility gate for the published release candidate.
8. Attach sanitized, count-only evidence to issue #73.
9. Remove this entry after the exit condition is true.

## Verification

Run the fixture generator check and the database release compatibility tests.
Record only module versions, object counts, invariant results, backup counts,
and test results. Confirm database format 7, read model 7, Codex usage 7, and
Claude usage 7. Confirm that known Codex and read-model version 6 sources each
create one backup, migrate to version 7 without losing retained rows, and
reopen idempotently. Confirm that the read-model migration adds the Profile
completion field with the pending default.

Do not record provider content, paths, identifiers, credentials, sessions, or
recovery material.

## Recovery

If a fixture cannot upgrade, stop before release. Keep the immutable source
fixture and coordinator backup. Fix the migration or invariant, then restart
the rehearsal from a new fixture copy. Do not change a stable fixture to hide
a migration failure.

## Cleanup targets

- the `releaseStatus` and `sourceCommit` fields for the `v0.0.10` entry in
  `apps/desktop/src-tauri/tests/fixtures/releases/manifest.json`; and
- `docs/cleanup/verify-issue-26-sqlite-schema-release.md`.

The released schema definitions and stable fixture history stay in the
database compatibility catalog.

## Exit condition

The release that contains the Codex and sanitized read-model version 7 schemas
is published, and issue #73 contains passing count-only evidence for every
acceptance criterion. The stable fixture bytes must remain unchanged, and the
second open must be idempotent.
