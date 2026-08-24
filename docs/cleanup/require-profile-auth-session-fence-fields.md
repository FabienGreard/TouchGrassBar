# Require the Profile Auth Session Fence Fields

- **Status:** rehearsed
- **Owner issue:** [#86](https://github.com/FabienGreard/TouchGrassBar/issues/86)
- **Implementation:** [PR #85](https://github.com/FabienGreard/TouchGrassBar/pull/85),
  [PR #87](https://github.com/FabienGreard/TouchGrassBar/pull/87)

## Scope

The `tokenmaxxers.activeAuthSessionId` and `authSessionGeneration` fields are
optional during one forward Convex migration. Existing Profile documents can
omit both fields. A successful Profile sign-in writes both fields before the
session can access protected data. Profile recovery clears the active session
and writes the transferred Active Mac generation.

The missing active session value is a denied state. The required schema will
use an explicit nullable value for that state. This entry does not authorize a
cloud migration or schema deployment.

## Execution plan

1. Widen `activeAuthSessionId` to an optional string-or-null field and deploy
   that compatibility schema through the approved workflow.
2. Add a bounded, resumable migration that sets a missing active session to
   `null` and copies the current Active Mac generation to a missing session
   generation.
3. Rehearse the migration on disposable, production-shaped local data. This
   step is complete.
4. Record the approved deployment before any remote action.
5. Run the migration in bounded batches until it completes.
6. Verify that no Profile document is missing either field and no Profile has
   invalid Active Mac authority.
7. Run the migration and the missing-field check again.
8. Change both schema fields from optional to required and keep the active
   session field nullable.
9. Remove the migration, compatibility checks, regression tests, maintenance
   text, and this entry in the same cleanup change.

## Verification

Record only the Profile document count, missing-field counts, invalid authority
count, migration change count, and repeated-run change count. The missing-field
counts, invalid authority count, and repeated-run change count must be zero. Do
not record Profile identifiers, session values, credentials, recovery material,
or other private values.

The local rehearsal used 121 disposable Profiles. It started with 61 missing
active Auth Session IDs, 61 missing Auth Session generations, and 91 Profiles
that required a change. One 25-Profile batch ran before an interruption. Four
more batches resumed from its cursor and completed the migration. The final
missing-field counts were zero. A second five-batch run changed zero Profiles.
The invalid Active Mac fixture reported one invalid authority and made no
change. This evidence is local only. It does not authorize a remote schema
deployment or migration.

## Recovery

Stop on a nonzero invalid authority count, an unexpected field value, or a
failed invariant. The migration writes only the explicit denied session state
and the existing Active Mac generation. It is safe to resume. Do not tighten
the schema until every approved deployment passes both checks.

## Cleanup targets

- the Profile auth session fence migration and its tests;
- `v.optional` around `activeAuthSessionId` and `authSessionGeneration` in
  `packages/backend/convex/schema.ts`;
- the missing-generation compatibility check in
  `packages/backend/convex/model/profile.ts`;
- the optional generated model types;
- the related migration text in `docs/backend.md`; and
- `docs/cleanup/require-profile-auth-session-fence-fields.md`.

## Exit condition

Every approved deployment has zero Profile documents without either field and
zero invalid Active Mac authorities. A repeated migration changes zero
documents. The required nullable schema passes local tests and the approved
deployment check.
