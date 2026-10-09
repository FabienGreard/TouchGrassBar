# Claude pricing recovery after incomplete scans

- **Status:** running
- **Owner issue:** [#112](https://github.com/FabienGreard/TouchGrassBar/issues/112)
- **Implementation:** [PR #113](https://github.com/FabienGreard/TouchGrassBar/pull/113); [parser-16 repair](https://github.com/FabienGreard/TouchGrassBar/commit/b8655ebaa60846454fd72f532a8af7cf8b7ff363).

## Scope

Desktop Claude daily aggregates with missing or stale pricing and retained scan
errors. The original aggregate fix changed no schema or parser version. Normal
scans can recover pricing from valid records already stored by the current
parser. The parser-16 checkpoint repair below is a separate bounded replay.

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

Release evidence on 2026-10-09:

- Candidate [a77c62a21238fc72f82d0ceff1e40a1d9e4d3d49](https://github.com/FabienGreard/TouchGrassBar/commit/a77c62a21238fc72f82d0ceff1e40a1d9e4d3d49)
  passed [main CI](https://github.com/FabienGreard/TouchGrassBar/actions/runs/37927242325)
  and the [release workflow](https://github.com/FabienGreard/TouchGrassBar/actions/runs/37927944017).
- Production deployment `next-pig-820` completed before `12:05:42Z`. The
  deployed backend includes the additive diagnostic fields before the new
  desktop release.
- At `12:06:23Z`, the production check verified one successful synchronization,
  four global reads, and four populated rows. It found no failures in the
  checked activity.
- The signed [v0.0.59 release](https://github.com/FabienGreard/TouchGrassBar/releases/tag/v0.0.59)
  became public at `12:20:28Z`. The public update-feed check passed.

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

[The detailed rejection change](https://github.com/FabienGreard/TouchGrassBar/commit/0955e467)
adds permanent `error_diagnostic_replay_v2` with the
same atomic cursor-reset and bounded scan behavior. The v1 marker stays in the
index; it cannot prevent the v2 scan. The replay regression starts with a v1
marker and retained failed files, then verifies field-level evidence, unchanged
known tokens and costs, unchanged complete-file cursors, and no second replay.

Owner issue #112 also tracks this rollout. Deploy the additive rejection
validator before shipping the new client. Verify an affected updated client
emits field/problem/counter-state codes. Do not close the remaining parser
investigation based only on pricing recovery. Keep both replay markers until
supported schema history makes their removal safe.

## Parser 16 checkpoint repair

Owner issue #112 also owns the parser-16 rollout. Local scanner tests found that
parser 15 could store a rejected prefix as `indexing` at a byte boundary. The
next pass could then mark the file complete without rechecking that prefix.
Parser 16 keeps the rejection in the existing `error` state while it resumes
the remaining bytes. It skips settled, unchanged error files and retains their
partial coverage. No table, column, or schema version changes.

The scanner regression failed before the fix: two bounded passes retained 100
valid tokens but incorrectly returned `complete`. It passed after the fix with
the same tokens and partial coverage. A second regression starts from a
parser-15 checkpoint with lost rejection state and verifies the bounded replay.
Repeat scans keep the daily revision and tokens unchanged. A corrected source
file can complete without recounting usage. The matching Codex scan test verifies
that its persisted rejection survives a bounded resume, keeps valid tokens,
and clears only after the source is corrected.

A second byte-boundary defect could parse the tail of an oversized rejected
physical line as a new record. Its regression counted 1,099 tokens before the
fix and 100 after it. Parser 16 discards the remaining bytes through the next
newline across bounded passes. It retains the valid next record and partial
coverage. The schema and existing checkpoint columns stay unchanged.

## Remote rejection dispositions

The complete seven-day receipt sample had 84 `invalid_message_metadata` reports
for `model` / `invalid_format` / zero counters and 60
`iteration_shape_mismatch` reports. These are report rows, not unique rejected
source records. Remote reports do not include the rejected values. The counts
cannot establish which source shape caused a rejection.

The metadata disposition is to keep unknown formats rejected. Existing tests
accept only the reviewed zero-usage API-error and synthetic-notice envelopes,
including their exact message, usage, and error metadata. They reject changed
model sentinels, unknown fields, changed iterations, incomplete HTTP error
metadata, and nonzero counters. The new scan regression uses a valid known
usage shape with an unreviewed version and a separate invalid-model zero-usage
record. It retains the valid 100 tokens, keeps coverage partial, reports only
bounded field/problem/counter codes, and stores no rejected source value.
Zero counters alone do not prove that an unknown record is a harmless notice.

The iteration disposition is to keep unmatched repetitions partial. Existing
tests accept a single reviewed iteration whose counters and cache split match
the outer usage, with an absent or matching optional model. They count the
outer usage once. Empty iterations, changed counters, unknown iteration fields,
and a conflicting or non-string model retain validated outer tokens with
partial coverage. Scanner tests retain 104 tokens from an empty-iteration
record across repeated scans and older-parser replay. They also verify the
bounded `iteration_shape_mismatch` report. Without rejected source values or
new provider evidence, the remote count does not justify marking these shapes
complete. Issue #112 keeps this source-shape investigation open.

The Codex checkpoint control test is green without a scanner behavior change:
a rejected prefix remains partial after a bounded resume, valid tokens remain,
and repeated scans keep the checkpoint stable. A corrected source clears the
error and completes without recounting usage.

Affected-client verification is still required. Let an affected updated client
complete its normal bounded replay and record count-only parser-16 evidence
in issue #112. Check that rejected records remain partial, valid token totals
remain, and unchanged checkpoints do not add revisions. A report with zero
token counters does not
prove that its rejected metadata is safe to accept.

If replay stops, retain the index and source files. The next normal scan resumes
its checkpoint. Do not delete stored usage, skip errors, or mark an unfinished
scan complete. Existing v1 and v2 diagnostic replay markers remain in place.
Remove this tracker only after the public release and affected-client evidence
satisfy the verification above. Local green tests do not prove live recovery.
