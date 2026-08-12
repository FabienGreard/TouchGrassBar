# Verify the Issue 26 SQLite Schema Release

- **Status:** planned
- **Owner issue:** [#73](https://github.com/FabienGreard/TouchGrassBar/issues/73)
- **Implementation:** [PR #65](https://github.com/FabienGreard/TouchGrassBar/pull/65)

## Scope

This tracker covers the first release that contains the issue 26 SQLite
schemas. It covers the database coordinator, sanitized read model, Codex usage
index, Claude usage index, and private usage synchronization ledger.

The release fixture rehearsal uses synthetic data only. It does not use a
cloud deployment or raw provider content.

## Execution plan

1. Generate the candidate release fixture with the current module versions.
2. Open each immutable stable fixture through the database coordinator.
3. Verify the upgraded module versions, object catalog, strict tables, foreign
   keys, and value invariants.
4. Open each upgraded fixture a second time and verify byte and backup
   idempotence.
5. Run the release compatibility gate for the published release candidate.
6. Attach sanitized, count-only evidence to issue #73.
7. Remove this entry after the exit condition is true.

## Verification

Run the fixture generator check and the database release compatibility tests.
Record only module versions, object counts, invariant results, backup counts,
and test results. Confirm database format 7, read model 6, Codex usage 6, and
Claude usage 7.

Do not record provider content, paths, identifiers, credentials, sessions, or
recovery material.

## Recovery

If a fixture cannot upgrade, stop before release. Keep the immutable source
fixture and coordinator backup. Fix the migration or invariant, then restart
the rehearsal from a new fixture copy. Do not change a stable fixture to hide
a migration failure.

## Cleanup targets

- this cleanup entry; and
- the candidate-only status for the `v0.0.9` fixture after the release commit
  and tag are final.

The released schema definitions and stable fixture history stay in the
database compatibility catalog.

## Exit condition

The release that contains PR #65 is published, and issue #73 contains passing
count-only evidence for every acceptance criterion. The stable fixture bytes
must remain unchanged, and the second open must be idempotent.
