# Claude pricing recovery after incomplete scans

- **Status:** rehearsed
- **Owner issue:** [#112](https://github.com/FabienGreard/TouchGrassBar/issues/112)
- **Implementation:** [PR #113](https://github.com/FabienGreard/TouchGrassBar/pull/113)

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
revisions. The native regression and compatibility tests pass; one existing test remains ignored.

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

## Codex catalog and model diagnostics rollout

The same patch adds `openai-standard-2026-09-24-v1`. Deploy its approved basis
and the optional unknown-model diagnostic field before the desktop release.
Keep `openai-standard-2026-09-05-v1` approved while older clients or unchanged
retained rows can send it. New model rates apply from 2026-09-22; earlier dates
stay unpriced. The normal repricing pass uses stored detail without an index
reset. Remove the older approved basis only after the retained history window
and supported clients no longer require it; track that removal in the owner
issue if it remains after this repair is verified.

## Failed-file diagnostic replay

The permanent `claude_usage_index_meta` marker `error_diagnostic_replay_v1`
records a one-time reset of error-file offsets for the new fixed parser codes.
The marker and cursor updates commit atomically. No message, aggregate, or
complete-file checkpoint is deleted. Normal byte/time budgets and checkpoints
bound the reread and permit resumption. The marker must remain until a later
schema migration or supported-version change proves that old error checkpoints
cannot return; do not delete it when closing this rollout entry.

The regression verifies that a retained failure produces its specific reason,
known tokens and prices stay unchanged, complete-file cursors stay unchanged,
and a repeated scan neither replays the failure nor adds a daily revision.
Live verification must record the new reason distribution if an updated client
still has rejected records. Do not declare its token gap fixed from a pricing
recovery alone.

## Detailed rejection replay

The next diagnostic change adds permanent `error_diagnostic_replay_v2` with the
same atomic cursor-reset and bounded scan behavior. The v1 marker stays in the
index; it cannot prevent the v2 scan. The replay regression starts with a v1
marker and retained failed files, then verifies field-level evidence, unchanged
known tokens and costs, unchanged complete-file cursors, and no second replay.

Owner issue #112 also tracks this rollout. Deploy the additive rejection
validator before shipping the new client. Verify an affected updated client
emits field/problem/counter-state codes. Do not close the remaining parser
investigation based only on pricing recovery. Keep both replay markers until
supported schema history makes their removal safe.
