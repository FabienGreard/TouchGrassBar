# Claude Opus 5.5 usage upgrade

- **Status:** running
- **Owner issue:** [#99](https://github.com/FabienGreard/TouchGrassBar/issues/99)
- **Implementation:** [v0.0.51](https://github.com/FabienGreard/TouchGrassBar/releases/tag/v0.0.51), commit `42c3deba605e567713434a154fe67cdb998b959f`

## Scope

Claude parser 14 reviews Claude Code 2.1.278, 2.1.280, and the nullable iteration model
field. The bundled price catalog adds Opus 5.5 Standard and Fast rates from
its September 22 launch. Normal bounded indexing replays older local
checkpoints and prices retained usage. There is no SQLite or cloud schema
change, temporary compatibility path, or cloud backfill.

## Execution plan

1. Verify synthetic current-CLI usage, duplicate records, pricing modifiers,
   optional iteration model metadata, parser-13 replay, and repeated scans.
2. Replay a private copy of the installed database with the candidate.
3. Compare Opus 5.5 token totals and cost with independent source arithmetic.
4. Verify all released database fixtures and run the required quality checks.
5. Publish and install the signed patch. Check the retained Claude index,
   current usage and cost, production synchronization, and public update feed.
6. Record aggregate evidence and remove this entry when its exit condition holds.

## Verification

The new regressions failed before the CLI review and parser revision update:
Opus 5.5 records stayed partial and unpriced. The synthetic pricing fixture
counts each outer counter once, including output that already contains thinking.
The parser-13 fixture must gain a priced estimate without changing its tokens,
and the second pass must not change the daily revision again.

The final candidate replayed a fresh installed-database copy. All 22 available
Claude files completed on parser 14, with zero pending or error files. Two
older checkpoints refer to missing source files and remain marked missing.
Repeated passes read zero additional bytes and kept the same daily values.

Independent arithmetic matched 39 unique Opus 5.5 messages: 4,935,919 tokens
and USD 2.5278392 API equivalent. Four earlier Opus 5 messages added 175,883
tokens and USD 0.2245435. The current-day total was 5,111,802 tokens and
USD 2.7523827, with complete coverage. No source usage copies conflicted.
The direct Claude Code 2.1.280 quota probe returned both quota windows.
Validation passed 756 native tests, 523 JavaScript tests, 48 database fixtures,
Clippy, formatting, contract checks, repository quality, and production builds.

The signed v0.0.50 draft passed token-count checks, but live verification found
that Today inherited a combined catalog label from older retained days. The
sync validator correctly rejected that label and omitted the Claude cost.
The draft remains unpublished and its immutable tag stays unchanged. Its
synthetic candidate fixture was replaced explicitly by the v0.0.51 candidate;
no official fixture was changed.

The correction gives Today the exact pricing basis stored for its UTC day.
Missing daily pricing metadata leaves the cost unavailable. Regression checks
cover retained old/new catalogs, missing daily metadata, and the outgoing
Claude daily aggregate. The wider period summaries retain their existing
combined provenance.

The v0.0.51 signed app passed the real retained-data checks before and after
publication. Its exact commit passed [CI run 35839312752](https://github.com/FabienGreard/TouchGrassBar/actions/runs/35839312752).
[Release run 35840127941](https://github.com/FabienGreard/TouchGrassBar/actions/runs/35840127941)
passed signing, notarization, Gatekeeper, updater signature checks, and all
48 database fixtures. All seven public downloads match the verified signed
draft bytes. The stable update feed names v0.0.51 and its signed archive.

After restart, independent arithmetic still matched 5,111,802 Claude tokens
and USD 2.7523827. The app, accepted upload, and public leaderboard agreed on
250,216,305 combined tokens and the API-equivalent cost. The current Claude
upload includes 2,752,383 cost micros with the exact daily pricing basis.
There were zero pending uploads or older available Claude checkpoints.
The production check recorded one successful synchronization, seven successful
reads, and zero failed executions after publication. The signed app also read
both quota windows from Claude Code 2.1.280.

The verified v0.0.51 app is running from the downloaded release. The installed
Applications copy is still v0.0.49. Computer cannot complete the replacement
until a Finder window is open. Keep this entry until that last step and its
installed-app checks pass; then record the result and remove it.
The broader provider audit still identifies the unreviewed stable CLI channel
and Codex changes; this release does not mark that full audit complete.

## Recovery

The replay uses the existing transactional, bounded scanner. A restart resumes
its work. Preserve the source transcripts. Do not change the installed database
by hand or change version markers to force old code to open it.

## Cleanup targets

Remove only this execution record after verification. Keep the parser review,
price catalog, regression fixtures, and nullable-model validation permanently.
The broader provider audit issue remains open for its other review items.

## Exit condition

The signed patch is public and installed. No retained Claude checkpoint waits
on an older parser. Opus 5.5 local tokens and cost match independent evidence,
the selected daily aggregates synchronize, and the stable feed names the patch.
Existing invalid historical records remain explicitly partial.
