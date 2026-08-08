# Issue 67: Retire Legacy Active Mac Authority

- **Status:** planned
- **Owner issue:** [#67 Retire legacy Active Mac authority](https://github.com/FabienGreard/TouchGrassBar/issues/67)
- **Implementation:** [PR #65](https://github.com/FabienGreard/TouchGrassBar/pull/65)
- **Migration:** `internal/migrations:retireLegacyActiveDeviceAuthority`

## Scope

The legacy Active Mac format stores `devices.installationId` without a proved
installation credential digest or Active Mac generation. The current format
stores a digest of the Keychain-held installation credential and a server-owned
generation.

The migration examines a Tokenmaxxer's linked Active Mac. For an exact legacy
row, it removes the legacy installation identifier, revokes that device, and
clears the Profile's Active Mac link. The next live Profile session can then
claim credential-based Active Mac authority.

No cloud deployment has run this migration from PR #65.

## Execution plan

1. Rehearse interruption and resume behavior on disposable,
   production-shaped data.
2. Capture a count-only preflight for exact eligible legacy Active Mac rows.
3. Obtain explicit authorization for the target deployment and migration run.
4. Run the migration through the Convex migrations component.
5. Resume the same migration if execution stops before completion.
6. Run the authenticated Active Mac claim canary after migration completion.
7. Complete the cleanup targets in a later schema change.

## Verification

The completion evidence must prove all of these invariants without exposing an
installation identifier or credential:

- no eligible legacy Active Mac row remains;
- each affected legacy device is revoked and has no `installationId`;
- each affected Tokenmaxxer has no link to the revoked legacy device;
- a live, matching Profile can claim a credential-based Active Mac; and
- the old device cannot synchronize Daily Usage Aggregates.

Local tests prove the migration logic, but they do not prove execution against
a cloud deployment.

## Recovery

The migration is forward-only and idempotent. Resume it after interruption.
Do not restore the legacy installation identifier. Affected Profiles recover by
claiming a new credential-based Active Mac through the governed Profile flow.

## Cleanup targets

After every supported deployment satisfies the verification invariants:

- remove `devices.installationId` from `convex/schema.ts`;
- remove the `by_tokenmaxxer_id_and_installation_id` index;
- remove `retireLegacyActiveDeviceAuthority`;
- replace migration-specific legacy fixtures with a retained schema-rejection
  test where useful; and
- update `docs/backend.md` so it describes the completed authority format and
  no longer presents this migration as pending.

## Exit condition

Delete this entry after the migration has completed on every supported
deployment, sanitized evidence is linked from issue #67, all cleanup targets
are merged, and the post-cleanup schema contains no legacy installation ID.
