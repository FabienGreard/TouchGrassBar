# Claude pricing recovery

- **Status:** running
- **Owner issue:** [#99](https://github.com/FabienGreard/TouchGrassBar/issues/99)
- **Implementation:** Claude parser 15 and catalog `anthropic-standard-2026-09-23-v2` in [commit 06a8ce25](https://github.com/FabienGreard/TouchGrassBar/commit/06a8ce2567884dae878b157e5c505748dd33f412).

## Scope

The local Claude index replays retained older checkpoints through its existing
bounded scanner. Valid counters no longer depend on a reviewed CLI version.
Compatible output snapshots keep the final price. Missing speed uses a modeled
standard rate. Published rates, SQLite schemas, and token categories do not
change. The generated contract accepts the new catalog and keeps the prior
catalog for unchanged days and older clients.

## Execution plan

1. Run native, contract, backend, and database compatibility checks.
2. Replay a private copy of retained data. Verify tokens, recovered prices,
   daily revisions, and an unchanged second scan.
3. In the next authorized release, deploy the backend contract before the
   desktop app sends the new pricing basis.
4. Let the released app complete its bounded scan. Verify that daily costs
   with both catalog versions synchronize and that repeated refresh adds no
   tokens or revisions.
5. Record sanitized release evidence and remove this entry.

## Verification

The new version, stream, cache-duration, replay, and missing-speed regressions
failed before the fix and passed after it. A separate regression caught the
need to accept the prior catalog when an unchanged day keeps its stored basis.

Validation passed 757 native library tests, 6 CLI tests, 86 backend tests,
22 contract tests, and 33 provider-audit tests. One existing native test is
ignored. All 48 SQLite release fixtures passed. Clippy, Rust and repository
formatting, generated contracts, and backend and contract type checks passed.

A private installed-database copy retained the same token totals on all 14
days. Only September 3 changed: 44,594 previously unpriced tokens gained a
modeled standard-rate cost of USD 0.041712. That day's revision increased once.
All 22 available files completed on parser 15. The second scan read no bytes
and changed no daily values or revisions. Today remained 5,111,802 tokens and
USD 2.7523827. The 30-day estimate became USD 142.947829 with all observed
tokens priced. Two older missing source files remain subject to the existing
partial-coverage rule.

The installed database was opened read-only to make the copy. The rehearsal
did not replace the installed app or repair live data.

## Release verification

[v0.0.52](https://github.com/FabienGreard/TouchGrassBar/releases/tag/v0.0.52)
is public. Candidate commit `8ed811ef7043d539a1a2563f4cda10ba21dc2c28`
passed [CI run 35854770934](https://github.com/FabienGreard/TouchGrassBar/actions/runs/35854770934).
The exact backend passed a dry deployment and deployed to production
`next-pig-820` before tag creation. The post-deployment check recorded two
successful usage synchronizations, ten successful leaderboard reads, and no
failed executions. A separate read returned 33 populated rows, including 21
rows with cost.

[Release run 35855475114](https://github.com/FabienGreard/TouchGrassBar/actions/runs/35855475114)
passed all 49 database fixtures, signing, notarization, Gatekeeper, and updater
signature checks. All seven public downloads match the verified draft hashes.
The downloaded updater archive passed a separate signature check. The stable
update feed names version 0.0.52 and its signed archive.

This release did not replace the installed app. Keep this entry until the
released app completes its local replay and both retained and new daily
pricing bases synchronize without duplicate revisions. The production checks
above verify backend availability and synchronization from existing clients;
the new parser's retained-data evidence remains the private-copy rehearsal.

## Recovery

Keep the source transcripts and existing database. An interrupted replay
resumes on the next scan. Do not reset a profile or edit parser markers in an
installed database. If the new pricing basis is rejected, verify the backend
contract deployment before publishing or retrying the desktop release.

## Cleanup targets

Remove this execution record after release verification. Keep the parser,
pricing rules, prior-catalog acceptance, and regression tests.

## Exit condition

An authorized release has completed the local replay and synchronized both
retained and new daily pricing bases without changing token totals or adding
duplicate revisions. Record the release and aggregate evidence before removal.
