# Claude pricing recovery after incomplete scans

- **Status:** rehearsed
- **Owner issue:** [#112](https://github.com/FabienGreard/TouchGrassBar/issues/112)
- **Implementation:** branch `codex/claude-partial-pricing-recovery`; pull request linked in the owner issue

## Scope

Desktop Claude daily aggregates with missing or stale pricing and retained scan
errors. The fix changes no schema or parser version. Normal scans can recover
pricing from valid records already stored by the current parser.

## Execution plan

1. Merge the tested fix and ship a desktop patch through the release workflow.
2. Let an affected client update and run its normal scan and synchronization.
3. Check the client version and daily pricing in production diagnostics. Record
   sanitized counts and coverage in the owner issue.

## Verification

The regression failed before the fix and passed after it. Tests cover parser
replay, absent aggregate parser metadata, unchanged current-parser checkpoints,
retained token totals, and growth of the priced subset. Repeat scans do not add
revisions. All 769 native tests pass; one existing test remains ignored.

Live recovery still needs evidence from an updated affected client: previously
unpriced daily usage gains priced tokens and a cost, known token totals remain,
and invalid records keep coverage partial. A released build alone is not proof
of client recovery.

## Recovery

Keep the local index and rejected records. If the scan is interrupted, the next
normal scan can resume. Do not delete the index or mark incomplete usage as
complete. If pricing does not improve, inspect the new diagnostic summary before
changing the parser or stored data.

## Cleanup targets

Delete this tracking file after verification. Keep the aggregate rule, regression
tests, ADR, and diagnostic guidance. There are no temporary fields or indexes.

## Exit condition

The patch is public and an affected updated client has synchronized recovered
pricing with the verification invariants above. Record evidence in issue #112
before closing it and removing this entry.
