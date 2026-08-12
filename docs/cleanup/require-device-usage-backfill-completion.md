# Require the Device Usage Backfill Completion Field

- **Status:** planned
- **Owner issue:** [#27](https://github.com/FabienGreard/TouchGrassBar/issues/27)
- **Implementation:** [issue #27](https://github.com/FabienGreard/TouchGrassBar/issues/27)
  pending an exact commit link in this change

## Scope

The `devices.usageBackfillCompletedAt` field is optional during one forward
Convex migration. Existing Device documents without the field use the same
pending state as an explicit `null` value. New Device documents always write
`null` before the first Profile backfill completes.

This entry does not authorize a cloud migration or schema deployment.

## Execution plan

1. Rehearse the migration on disposable, production-shaped local data.
2. Record the approved deployment before any remote action.
3. Run `backfillDeviceUsageCompletion` in bounded batches until it completes.
4. Verify that no Device document is missing the field.
5. Run the migration and the missing-field check again.
6. Change the schema field from optional to required.
7. Remove the migration, compatibility read, regression, maintenance text,
   and this entry in the same cleanup change.

## Verification

Record only the number of Device documents, missing fields, migration changes,
and repeated-run changes. The missing-field count and repeated-run change count
must both be zero. Do not record document IDs, credentials, sessions, Profile
data, or other private values.

## Recovery

Stop on an authorization error, an unexpected value, or a failed invariant.
The migration writes only `null` to a missing field and is safe to resume. Do
not tighten the schema until every approved deployment passes both checks.

## Cleanup targets

- `backfillDeviceUsageCompletion` in
  `packages/backend/convex/internal/migrations.ts`;
- `v.optional` around `usageBackfillCompletedAt` in
  `packages/backend/convex/schema.ts`;
- the missing-field compatibility checks in `assertHistoricalAdmission` and
  `applyUsageSnapshots` in `packages/backend/convex/model/sync.ts`;
- `the device completion migration preserves pending Profile authority` in
  `packages/backend/convex/sync.test.ts`;
- the Device completion migration text in `docs/backend.md`; and
- `docs/cleanup/require-device-usage-backfill-completion.md`.

## Exit condition

Every approved deployment has zero Device documents without the field. A
repeated migration changes zero documents. The required schema passes local
tests and the approved deployment check.
