# Codex response accounting upgrade

- **Status:** rehearsed
- **Owner issue:** [#95](https://github.com/FabienGreard/TouchGrassBar/issues/95)
- **Implementation:** patch candidate v0.0.49, following [v0.0.48](https://github.com/FabienGreard/TouchGrassBar/releases/tag/v0.0.48)

## Scope

SQLite format 9 and Codex index version 11 add the permanent nullable
`codex_usage_files.response_cursor` field. Parser 25 records independent
response usage, including compaction. The field stores the response checkpoint
and the dates of known response gaps. It contains no prompts or response text.

Parser 25 rebuilds all retained older file checkpoints through the bounded
scanner. The former parser-18 through parser-23 promotion shortcut is removed
in this change: historical counts can also contain repeated resume history or
omit compaction. Keeping that shortcut for a retention window would preserve
incorrect counts. The old cleanup entry is replaced by this execution record.

This migration changes local SQLite data. The release also deploys the matching
backend validator, which accepts a larger Codex local total at the same account
observation time. It does not change the cloud schema or require a cloud backfill.

## Execution plan

1. Verify upgrades and repeated opens for every published database fixture.
2. Replay an isolated copy of the installed database with the candidate parser.
3. Compare the completed current-day index and hourly sum with an independent
   sum of unique response records, at the same saved file offsets.
4. Verify restart, duplicate response, copied-parent, compaction, legacy
   transition, UTC boundary, partial-day, and correction behavior.
5. Publish the signed patch through the release skill. Check the installed app,
   local synchronization ledger, production read, and public update feed.
6. Record the release evidence and remove this entry after the exit condition.

## Verification

The regression loop reproduced both the delayed-account replacement and the
missing response accounting on the previous code. The candidate passes both.
The database suite upgrades and reopens all 47 fixtures. Separate tests verify
that invalid saved response state fails closed and that the version-10 upgrade
preserves checkpoints before the bounded replay begins.

The first independent current-day comparison counted 4,462 unique responses
and 662,067,049 tokens across 46 source files. Every per-file count matched the
candidate index. There were zero duplicate response identifiers and zero
conflicting records. An earlier gap in a file must keep only the affected UTC
days partial, even if the file receives new responses today.

The final candidate current-day comparison counted 4,574 unique responses and
681,315,018 tokens. Every per-file total matched, with zero duplicates,
conflicts, or current-day error checkpoints. Current-day coverage was complete.
The replay of older retained files is still in progress. Full local validation
passed: 751 native tests, 521 JavaScript tests, 47 database fixtures, type checks,
Clippy, formatting, production builds, and the local Convex deployment.

Installed-release evidence will be added after signed-candidate verification.
Store only aggregate counts and public release links here.

## Recovery

The coordinator creates and checks a source backup before migration. The
migration and each bounded indexing pass are transactional and resumable.
The old app rejects the newer database format. Recover through the supported
database backup procedure if a forward migration cannot complete. Do not run
an old binary against a database whose version marker was manually lowered.

## Cleanup targets

The response checkpoint, source-selection rule, correction marker, migration,
and regression tests are permanent. Remove only this execution record after
all release evidence is recorded. No temporary read or write path remains.

## Exit condition

The signed patch is public, its installed database has zero retained older
parser checkpoints and zero pending checkpoints, its current-day local and
hourly totals match independent response evidence, and its leaderboard matches
the selected local total. Historical malformed source records remain explicitly
partial; they must not be described as recovered without valid source evidence.
