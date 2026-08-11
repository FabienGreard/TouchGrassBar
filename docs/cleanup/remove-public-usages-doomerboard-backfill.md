# Remove the Public Usages Doomerboard Backfill

- **Status:** planned
- **Owner issue:** [#74](https://github.com/FabienGreard/TouchGrassBar/issues/74)
- **Implementation:** [PR #65](https://github.com/FabienGreard/TouchGrassBar/pull/65)

## Scope

This tracker covers the bounded `backfillDoomerboard` repair. The repair adds
missing Doomerboard index entries from `publicUsages`. It does not recalculate
usage or Token Scores.

Only an explicitly approved deployment is in scope. PR #65 does not run a
cloud migration or change a cloud deployment.

## Execution plan

1. Rehearse the repair on disposable, production-shaped local data.
2. Record the approved deployment scope before a remote action.
3. Run the read-only one-to-one invariant for `publicUsages` and Doomerboard.
4. Run the resumable backfill only if the invariant reports missing entries.
5. Resume bounded batches until the migration reports completion.
6. Run the invariant and the migration a second time.
7. Remove the repair after all approved deployments meet the exit condition.

## Verification

Record only the number of Public Usages, index entries, missing entries, extra
entries, and mismatched entries. Record the number of changes from each
migration run. The second run must change zero entries.

Do not record display names, TouchGrass IDs, document IDs, credentials,
sessions, deployment secrets, or other private identifiers.

## Recovery

Stop on an authorization error, invariant mismatch, or unexpected write. The
repair inserts missing entries only, so an interrupted run can resume. Do not
make manual dashboard edits. Use a separate reviewed repair for extra or
mismatched entries.

## Cleanup targets

- `backfillDoomerboard` in `packages/backend/convex/internal/migrations.ts`;
- its repair regression in `packages/backend/convex/sync.test.ts`;
- its maintenance text in `docs/backend.md`;
- generated Convex declarations that reference the removed function;
- the migrations component and package if no other migration uses them; and
- this cleanup entry.

## Exit condition

Every approved deployment has zero missing, extra, and mismatched index
entries. A repeated backfill changes zero entries. The cleanup change then
removes the repair and this entry.
