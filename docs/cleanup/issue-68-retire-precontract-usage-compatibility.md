# Issue 68: Retire Pre-contract Usage Compatibility

- **Status:** planned
- **Owner issue:** [#68 Retire pre-contract usage compatibility](https://github.com/FabienGreard/TouchGrassBar/issues/68)
- **Implementation:** [PR #65](https://github.com/FabienGreard/TouchGrassBar/pull/65)
- **Migrations:** `internal/migrations:upgradePrecontractUsageBuckets`,
  `upgradePrecontractUserDailyUsage`, `upgradePrecontractUserScores`, and
  `upgradePrecontractPublicScores`

## Scope

PR #65 replaces the pre-contract numeric cost fields with the current
`apiEquivalentCost` object and adds evidence and correction fields to Usage
Buckets. The schema keeps both shapes optional during the governed transition.
Current writes use only the new shape. They also remove old fields when they
replace a row.

No cloud deployment has run these migrations from PR #65.

## Execution plan

1. Rehearse all four migrations on disposable, production-shaped data.
2. Capture count-only preflight totals for rows that do not have the current
   required fields.
3. Obtain explicit authorization for the target deployment and migration run.
4. Run the four migrations in the listed order.
5. Resume a migration if execution stops before completion.
6. Verify the post-migration invariants.
7. Make the current fields required and remove the compatibility fields in a
   later pull request.

## Verification

The completion evidence must prove these invariants without provider content or
private identifiers:

- every Usage Bucket has `apiEquivalentCost`, `evidenceBasis`,
  `correctionReason`, and `correctionRevision`;
- every Daily Usage and score row has `apiEquivalentCost`;
- no row has `apiEquivalentCostMicros`, `costIsComplete`,
  `priceBasisVersion`, or the old Usage Bucket `source`; and
- a new Codex and Claude synchronization request updates scores with the current
  cost shape.

Local tests prove the migration logic. They do not prove execution against a
cloud deployment.

## Recovery

The migrations are idempotent and use row replacement. Resume them after an
interruption. A row without the current cost shape is excluded from score
calculation until a migration or a current synchronization write replaces it.
The migration does not infer a cost from the old numeric field.

## Cleanup targets

After every supported deployment satisfies the verification invariants:

- make the current Usage Bucket fields required in `convex/schema.ts`;
- make `apiEquivalentCost` required in Daily Usage and score tables;
- remove the optional `apiEquivalentCostMicros`, `costIsComplete`,
  `priceBasisVersion`, and old Usage Bucket `source` fields;
- remove the four `upgradePrecontract*` migration functions;
- replace migration-specific fixtures with retained schema-rejection tests where
  useful; and
- update `docs/backend.md` so it no longer describes a staged transition.

## Exit condition

Delete this entry after the migrations have completed on every supported
deployment, sanitized evidence is linked from issue #68, all cleanup targets
are merged, and the strict schema accepts the retained deployment data.
