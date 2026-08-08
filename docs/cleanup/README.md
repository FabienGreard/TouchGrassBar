# Migration and Cleanup Tracker

`docs/cleanup` tracks active work that must happen after a data migration,
temporary compatibility bridge, staged schema change, or bounded repair.
GitHub Issues own the work. The files in this folder make the required
execution and removal steps visible in the repository.

## When an entry is required

Add one Markdown file before merging a change that introduces any of these
items:

- a data migration or backfill;
- a temporary field, index, read path, write path, or dual-write bridge;
- a staged schema transition;
- a repair that must run against a deployment; or
- code that can be removed only after deployment evidence exists.

Do not create an entry for cleanup that the same change can complete safely.
Complete that cleanup in the change instead.

## Required fields

Each entry must contain:

- **Status:** planned, rehearsed, running, blocked, or ready to remove;
- **Owner issue:** one open GitHub issue;
- **Implementation:** the pull request or commit that introduced the work;
- **Scope:** the data, schema, compatibility code, and deployments in scope;
- **Execution plan:** ordered, resumable steps;
- **Verification:** count-only or invariant evidence that proves completion;
- **Recovery:** the safe response to interruption or failure;
- **Cleanup targets:** exact fields, indexes, functions, tests, and documents to
  remove or update; and
- **Exit condition:** one checkable condition that permits removal of the entry.

Never store credentials, sessions, recovery material, raw provider content,
private paths, deployment secrets, or private identifiers in an entry. Link to
sanitized CI, issue, or deployment evidence instead.

## Lifecycle

1. Create the entry with a planned status and an open owner issue.
2. Rehearse the migration on disposable, production-shaped data.
3. Record sanitized evidence and the exact approved deployment scope.
4. Run or resume the migration through its governed deployment workflow.
5. Verify the post-migration invariants before changing reads or removing
   compatibility code.
6. Remove the temporary schema and code in a later change when required.
7. Delete the cleanup entry after its exit condition is true. Git history and
   the owner issue retain the completed record.

A migration is not complete only because its function returned successfully.
Its invariants, affected deployment scope, and cleanup targets must also be
complete.

## Active entries

- [Issue 67: retire legacy Active Mac authority](./issue-67-retire-legacy-active-device-authority.md)
- [Issue 68: retire pre-contract usage compatibility](./issue-68-retire-precontract-usage-compatibility.md)

## Entry template

```markdown
# Cleanup title

- **Status:** planned
- **Owner issue:** [#000](https://github.com/OWNER/REPO/issues/000)
- **Implementation:** PR or commit link

## Scope

## Execution plan

## Verification

## Recovery

## Cleanup targets

## Exit condition
```
