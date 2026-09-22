# Remove Codex Parser-18 through Parser-23 Usage Index Compatibility

- **Status:** rehearsed
- **Owner issue:** [#95](https://github.com/FabienGreard/TouchGrassBar/issues/95)
- **Implementation:** [commit c1f0061f](https://github.com/FabienGreard/TouchGrassBar/commit/c1f0061fc9a99914a97ff888bb7079a02f1abf68), parser 19, and [the parser-20 update in PR #106](https://github.com/FabienGreard/TouchGrassBar/pull/106), and [the parser-23 recovery](https://github.com/FabienGreard/TouchGrassBar/commit/768ca6942a7ec4db55f848ac30ae745c0ddfa0f1)

## Scope

The local Codex usage index promotes only complete, included, supported, and
error-free parser-18 through parser-23 rows that have no current-day usage.
The current parser must replay current-day files to create hourly detail.
The bounded scanner replaces daily and hourly rows together, so repeated
passes do not add the same tokens twice.

Parser 23 accepts the optional `metadata` field on response records without
using its contents as token evidence. It replays older error checkpoints,
including forked tasks whose usage parser 22 excluded after a header error.
Error checkpoints cannot use the promotion shortcut. This recovery uses the
existing bounded replay path and introduces no database schema change.

Parser 24 accepts the optional `root_turn_id` field on `task_started` events.
It does not store the field or use it as token or pricing evidence. It replays
parser-23 error checkpoints so rejected task starts can restore counter reset
boundaries. Complete historical checkpoints can use the existing promotion
path. Current-day checkpoints replay to restore hourly detail.

SQLite format 8 and Codex index version 10 add the permanent
`codex_usage_file_model_hours` table. The forward migration creates the table
inside the existing backup and transaction protocol. It does not fill hours
from daily totals. The parser fills the current day from the source records.
Older hours are removed in batches of `PRUNE_ROWS_PER_PASS`.
The stored-cursor recovery, separate discovery and parse budgets, provider
database-writer coordination, and rollout path containment are permanent.
They are not cleanup targets.

The local schema upgrade and parser backfill are in scope. No cloud deployment is in scope.

## Execution plan

1. Keep the compatibility bridge for one full 60-day Codex usage-retention
   window after the parser-24 release is available.
2. Copy a production-shaped local database to an isolated test location. Do
   not change the installed application database.
3. Run one normal indexing pass against the isolated database and rollout
   source.
4. Verify that current-day files replay rather than use the promotion shortcut.
   Check that hourly tokens match accepted local daily tokens and that missing
   detail remains unavailable until the scan completes. Record the retained
   parser-18 through parser-23 row counts and the count of rows promoted by
   the pass.
5. Run a second normal pass and record the same counts.
6. Remove the compatibility bridge and its promotion-only tests in a later
   pull request.
7. Run the Codex usage tests, the full native suite, Clippy, and
   `bun run quality`.
8. Remove this entry in the same cleanup change.

## Verification

Use count-only evidence from the isolated database. After the first pass,
there must be zero retained parser-18 through parser-23 rows. The second pass
must promote zero additional rows. The retained window must also have zero
non-current parser rows in a pending or error state before the bridge is removed.

Local rehearsal: the synthetic upgrade test starts with parser-20 file rows
and an empty hour index. The first replay restores 300 tokens in two hours.
A repeated pass retains 300 tokens. The database suite upgrades and reopens
all stored release fixtures, and the candidate fixture contains the new table.
Release and installed-device execution evidence is still required before
removing the bridge.

Parser-23 regression tests cover fresh forked tasks and parser-22 error
checkpoints, both on the current day and on a previous day. Replay restores
100 local tokens after an inherited baseline. Repeated passes retain 100
tokens, and current-day hourly detail also totals 100 tokens. Response
metadata does not enter the sanitized usage report.

Local rehearsal on 2026-09-16 used two unchanged, isolated record copies.
Parser 22 left both files in an error state and selected zero tokens. Parser
23 replayed those saved checkpoints and completed both files without errors.
Their current-day totals were 111,954,846 and 63,259,847 tokens. Repeated
passes kept both totals unchanged. The full native suite passed, including
release database compatibility tests, and Clippy reported no warnings.

A separate rehearsal replayed a copy of the installed database. All 2,208
file checkpoints reached parser 23, with zero older parser checkpoints and
zero pending files. Error checkpoints fell from 80 to 9. The remaining
failures involved existing size or cumulative-token validation; none had
current-day indexed tokens. The installed database was not changed.

The parser-24 rehearsal on 2026-09-22 replayed an isolated database copy.
All 2,266 file checkpoints reached parser 24, with zero older checkpoints
and zero pending files. Error checkpoints fell from 64 to 9. The remaining
errors involved existing size or cumulative-token checks. The current-day
scan completed. Repeated passes kept its 490,599,138 local tokens unchanged.
The daily selection used this count in place of the omitted account day's
older 22,640,561 tokens. Tests also proved that a returned account bucket,
including a lower correction or zero, takes priority again. The full native
suite, Clippy, and formatting checks passed. The installed database was not
changed. Release and installed-device verification remain pending.

The local native app was also checked with Computer on 2026-09-22. It used
the local backend and a separate development database. Codex Today showed
510,425,117 tokens, equal to the sum of its hourly chart. The seven-day total
of 1,791,641,187 and the 30-day total of 14,140,881,894 each matched the sum
of their daily chart. Manual refresh returned to Live. Its 30-day total of
14,142,583,046 still matched the chart. The current-day count increased as
new usage arrived. A stored snapshot also matched its hourly index total of
512,068,968. The final scan had zero pending files and nine existing error
files. Today was complete; the older periods remained partial. Temporary
window settings used for the Computer check were removed.

Do not record rollout content, paths, prompts, session identifiers,
credentials, or other private values.

## Recovery

Keep the bridge if any required count is not zero or if the isolated pass does
not complete. The pass is resumable and changes only the isolated database.
If removal later causes upgrade indexing to regress, restore the bounded
promotion query and its strict safety guards.

## Cleanup targets

- `COMPATIBLE_ROLLOUT_PARSER_VERSION` in `usage.rs`;
- `promote_compatible_parser_rows` in `usage.rs`;
- the `compatible_parser_rows_promoted` event and its indexing-pass call;
- `sqlite_index_reuses_complete_rows_from_the_previous_compatible_parser`;
- `sqlite_index_does_not_promote_unsafe_previous_parser_rows`;
- previous-parser setup in
  `sqlite_index_retries_files_rejected_by_the_previous_codex_parser`;
- previous-parser setup in
  `sqlite_index_resumes_a_stored_current_parser_cursor_after_discovery_times_out`;
  and
- `docs/cleanup/remove-codex-parser-18-compatibility.md`.

## Exit condition

After one full 60-day retention window, one isolated normal pass leaves zero
retained parser-18 through parser-23 rows and zero non-current pending or error
rows. A repeated pass promotes zero rows, and the cleanup change passes every
required local and CI check.
