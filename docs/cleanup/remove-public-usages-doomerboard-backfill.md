# Remove the Public Usages Doomerboard Backfill

- **Status:** planned
- **Owner issue:** [#74](https://github.com/FabienGreard/TouchGrassBar/issues/74)
- **Implementation:** [PR #65](https://github.com/FabienGreard/TouchGrassBar/pull/65)
  and [commit 81a1013c](https://github.com/FabienGreard/TouchGrassBar/commit/81a1013c9b996553e87ddf0b0099edc65970b910)

## Scope

This tracker covers the bounded `backfillDoomerboard` repair. The repair
removes a legacy numeric Aggregate key and inserts the deterministic composite
`[-TokenScore, TouchGrass ID]` key for each `publicUsages` row. It also adds a
missing entry. It does not recalculate usage or Token Scores.

Until the backfill and invariant checks complete, the Global Doomerboard keeps
a bounded compatibility read through the Public Usage score-order index and
the canonical Aggregate composite keys. The compatibility read completes only
its bounded score-boundary tie, so it does not scan a complete legacy tie.

Only an explicitly approved deployment is in scope. PR #65 does not run a
cloud migration or change a cloud deployment.

## Execution plan

1. Rehearse the repair on disposable, production-shaped local data.
2. Record the approved deployment scope before a remote action.
3. Run the read-only one-to-one invariant for `publicUsages` and Doomerboard.
4. Run the idempotent repair if the invariant reports a missing, extra, or
   mismatched entry. Run the resumable backfill for the legacy numeric-key
   migration.
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

Stop on an authorization error or unexpected write. Each repair mutation
deletes only its observed Aggregate entry, reads the current Public Usage,
corrects its board key from its scope and window when required, and inserts the
canonical composite key. The backfill deletes only the matching legacy numeric
key before it inserts that composite key. Both paths can resume. Do not make
manual dashboard edits.

## Cleanup targets

- `backfillDoomerboard` in `packages/backend/convex/internal/migrations.ts`;
- the legacy numeric key type and validator in
  `packages/backend/convex/model/doomerboard.ts`;
- the bounded legacy numeric-key read path and regression in
  `packages/backend/convex/doomerboards.ts`, the temporary
  `by_board_key_and_token_score_and_touch_grass_id` index in
  `packages/backend/convex/schema.ts`, and the regression in
  `packages/backend/convex/sync.test.ts`;
- the legacy numeric-key deletion in
  `packages/backend/convex/model/scores.ts`;
- `doomerboardInvariantPage.repairEntry` and the `repair` action in
  `packages/backend/convex/internal/doomerboardInvariantPage.ts` and
  `packages/backend/convex/internal/doomerboardInvariant.ts` after the repair
  window closes;
- its repair regression in `packages/backend/convex/sync.test.ts`;
- its maintenance text in `docs/backend.md`;
- generated Convex declarations that reference the removed function;
- the migrations component and package if no other migration uses them; and
- this cleanup entry.

## Exit condition

Every approved deployment has zero missing, extra, and mismatched index
entries. A repeated backfill changes zero entries. The cleanup change then
removes the repair and this entry.
