# Remove Codex Parser-17 Usage Index Compatibility

- **Status:** planned
- **Owner issue:** [#95](https://github.com/FabienGreard/TouchGrassBar/issues/95)
- **Implementation:** [commit c1f0061f](https://github.com/FabienGreard/TouchGrassBar/commit/c1f0061fc9a99914a97ff888bb7079a02f1abf68)

## Scope

The local Codex usage index promotes only complete, included, supported, and
error-free parser-17 rows to parser 18. Rows that do not meet every guard use
the normal fail-closed reparse path.

The stored-cursor recovery, separate discovery and parse budgets, provider
database-writer coordination, and rollout path containment are permanent.
They are not cleanup targets.

No schema or cloud deployment is in scope.

## Execution plan

1. Keep the compatibility bridge for one full 60-day Codex usage-retention
   window after the hotfix release is available.
2. Copy a production-shaped local database to an isolated test location. Do
   not change the installed application database.
3. Run one normal indexing pass against the isolated database and rollout
   source.
4. Record the retained parser-17 row count and the count of rows promoted by
   the pass.
5. Run a second normal pass and record the same counts.
6. Remove the compatibility bridge and its promotion-only tests in a later
   pull request.
7. Run the Codex usage tests, the full native suite, Clippy, and
   `bun run quality`.
8. Remove this entry in the same cleanup change.

## Verification

Use count-only evidence from the isolated database. After the first pass,
there must be zero retained parser-17 rows. The second pass must promote zero
additional rows. The retained window must also have zero non-current parser
rows in a pending or error state before the bridge is removed.

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
- `docs/cleanup/remove-codex-parser-17-compatibility.md`.

## Exit condition

After one full 60-day retention window, one isolated normal pass leaves zero
retained parser-17 rows and zero non-current pending or error rows. A repeated
pass promotes zero rows, and the cleanup change passes every required local and
CI check.
