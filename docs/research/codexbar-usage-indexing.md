# CodexBar local Codex usage indexing

**Status:** Current-source research; no production code change

**Date:** 2026-08-06
**Scope:** Local Codex cost indexing in `steipete/CodexBar` at commit
[`50c5b002`](https://github.com/steipete/CodexBar/tree/50c5b00221ff5bff2f8582c8f6d1f434f428be41).
Only the official repository is used.

## Short answer

CodexBar does not wait for the next normal provider refresh to continue a large
first scan. The app gives the first scan a two-second wall-clock budget, saves
its cursor, then starts a persistent catch-up task that runs more two-second
passes. The normal mode spaces the passes to limit CPU and disk use; a **Finish
now** action runs them without a delay. All corpus scans use one serial utility
queue, so they do not block Swift's cooperative task pool and do not overlap.
([initial two-second limit](https://github.com/steipete/CodexBar/blob/50c5b00221ff5bff2f8582c8f6d1f434f428be41/Sources/CodexBarCore/CostUsageFetcher.swift#L1121-L1136),
[catch-up loop](https://github.com/steipete/CodexBar/blob/50c5b00221ff5bff2f8582c8f6d1f434f428be41/Sources/CodexBar/UsageStore%2BCodexCostCatchUp.swift#L101-L220),
[serial executor](https://github.com/steipete/CodexBar/blob/50c5b00221ff5bff2f8582c8f6d1f434f428be41/Sources/CodexBarCore/CostUsageScanExecutor.swift#L3-L14))

## Scheduling and background work

- Each bounded catch-up pass uses the same two-second scan limit as the first
  app refresh. The catch-up task is `.background` in automatic mode and
  `.utility` in accelerated mode. It does not cancel a pass while that pass can
  be writing a checkpoint. ([pass setup](https://github.com/steipete/CodexBar/blob/50c5b00221ff5bff2f8582c8f6d1f434f428be41/Sources/CodexBarCore/CostUsageFetcher.swift#L273-L299),
  [task setup](https://github.com/steipete/CodexBar/blob/50c5b00221ff5bff2f8582c8f6d1f434f428be41/Sources/CodexBar/UsageStore%2BCodexCostCatchUp.swift#L19-L59))
- Automatic catch-up targets a 20% duty cycle on AC power, 5% on battery, and
  15% when the power source is unknown. It pauses for 60 seconds in Low Power
  Mode or serious thermal pressure, and always pauses under critical thermal
  pressure. Accelerated mode runs the next pass immediately.
  ([policy](https://github.com/steipete/CodexBar/blob/50c5b00221ff5bff2f8582c8f6d1f434f428be41/Sources/CodexBar/CodexCostCatchUpPolicy.swift#L82-L115))
- The loop stops safely on cancellation, an error, or a pass that makes no
  cursor progress. After the final pass, it publishes one stable snapshot.
  ([stop and publication rules](https://github.com/steipete/CodexBar/blob/50c5b00221ff5bff2f8582c8f6d1f434f428be41/Sources/CodexBar/UsageStore%2BCodexCostCatchUp.swift#L166-L269))

## Cursor and persistence authority

CodexBar has two different persistence layers:

1. The **Codex v11 JSON cost cache** owns JSONL parsing state, token deltas,
   discovery state, and incremental cursors. Its per-file state includes source
   modification time and size, parsed byte offset, token baselines, file
   identity, target size, completion state, a partial-JSON-line resume state,
   and a token-index anchor. The cache also stores total processed bytes and
   files for progress. ([cache state](https://github.com/steipete/CodexBar/blob/50c5b00221ff5bff2f8582c8f6d1f434f428be41/Sources/CodexBarCore/Vendored/CostUsage/CostUsageCache.swift#L779-L855),
   [per-file cursor fields](https://github.com/steipete/CodexBar/blob/50c5b00221ff5bff2f8582c8f6d1f434f428be41/Sources/CodexBarCore/Vendored/CostUsage/CostUsageCache.swift#L1008-L1056))
2. The **Workspaces SQLite sidecar** stores normalized catalog, rollout, daily,
   event, snapshot, and index-state rows for project/session analytics. It does
   not reconstruct parser state and does not own the JSONL cursor. The official
   design document states that the cost scanner remains authoritative.
   ([design contract](https://github.com/steipete/CodexBar/blob/50c5b00221ff5bff2f8582c8f6d1f434f428be41/docs/codex-workspaces.md#L5-L24),
   [sidecar schema](https://github.com/steipete/CodexBar/blob/50c5b00221ff5bff2f8582c8f6d1f434f428be41/Sources/CodexBarCore/CodexWorkspaceUsageSidecar.swift#L358-L453))

The sidecar uses schema version 5 and snapshot payload format 3. Its tables are
`catalog_threads`, `usage_rollouts`, `usage_daily`, `usage_events`,
`snapshot_payloads`, and `index_state`. Writes use WAL mode and one immediate
transaction. A failed synchronization rolls back to the last complete
snapshot. ([versions and transaction](https://github.com/steipete/CodexBar/blob/50c5b00221ff5bff2f8582c8f6d1f434f428be41/Sources/CodexBarCore/CodexWorkspaceUsageSidecar.swift#L10-L15),
[open and WAL policy](https://github.com/steipete/CodexBar/blob/50c5b00221ff5bff2f8582c8f6d1f434f428be41/Sources/CodexBarCore/CodexWorkspaceUsageSidecar.swift#L335-L372),
[transactional synchronization](https://github.com/steipete/CodexBar/blob/50c5b00221ff5bff2f8582c8f6d1f434f428be41/Sources/CodexBarCore/CodexWorkspaceUsageSidecar.swift#L142-L178))

The sidecar imports only changed rollouts. Its source identity includes
modification time, size, parsed bytes, session identity, parser producer key,
pricing key, and a content fingerprint. Unchanged rows are only marked present;
changed rows are replaced. Missing rollouts are marked absent.
([source identity](https://github.com/steipete/CodexBar/blob/50c5b00221ff5bff2f8582c8f6d1f434f428be41/Sources/CodexBarCore/CodexWorkspaceUsageSidecar.swift#L17-L67),
[delta import](https://github.com/steipete/CodexBar/blob/50c5b00221ff5bff2f8582c8f6d1f434f428be41/Sources/CodexBarCore/CodexWorkspaceUsageSidecar.swift#L536-L578))

## Incremental append handling

For an unchanged file, CodexBar reuses the cached contribution. For an appended
file, it can resume at `parsedBytes` only when the source file identity still
matches and a SHA-256 anchor over the last 64 KiB of the indexed prefix still
matches. It also restores partial-line JSON state and token-counter baselines.
If these checks or fork-lineage rules fail, it performs a bounded full rescan.
([prefix anchor](https://github.com/steipete/CodexBar/blob/50c5b00221ff5bff2f8582c8f6d1f434f428be41/Sources/CodexBarCore/Vendored/CostUsage/CostUsageScanner.swift#L2033-L2076),
[append gates](https://github.com/steipete/CodexBar/blob/50c5b00221ff5bff2f8582c8f6d1f434f428be41/Sources/CodexBarCore/Vendored/CostUsage/CostUsageScanner%2BCacheHelpers.swift#L1111-L1199),
[rescan fallback](https://github.com/steipete/CodexBar/blob/50c5b00221ff5bff2f8582c8f6d1f434f428be41/Sources/CodexBarCore/Vendored/CostUsage/CostUsageScanner.swift#L4219-L4253))

The reader uses 256 KiB chunks. If a time or byte budget ends in the middle of
a JSONL record, it stores the byte offset, line start, retained prefix, line
length, truncation flag, and JSON tail state. The next pass continues instead
of starting that file again. ([bounded JSONL reader](https://github.com/steipete/CodexBar/blob/50c5b00221ff5bff2f8582c8f6d1f434f428be41/Sources/CodexBarCore/Vendored/CostUsage/CostUsageJsonl.swift#L295-L436))

## First-scan limits and order

The scanner defaults are 256 MiB for one rollout and 512 MiB of newly read data
per refresh. It prefers the newest sessions first, then persists incomplete
files, older-partition lookback state, and session-discovery cursors for later
passes. The app adds the two-second wall-clock limit described above.
([default budgets](https://github.com/steipete/CodexBar/blob/50c5b00221ff5bff2f8582c8f6d1f434f428be41/Sources/CodexBarCore/Vendored/CostUsage/CostUsageScanner.swift#L46-L92),
[newest-first ordering and persisted catch-up state](https://github.com/steipete/CodexBar/blob/50c5b00221ff5bff2f8582c8f6d1f434f428be41/Sources/CodexBarCore/Vendored/CostUsage/CostUsageScanner.swift#L4497-L4679))

The v11 JSON artifact is also bounded: 256 MiB and 25,000 entries when saved,
with a 320 MiB load refusal limit. CodexBar prunes rebuildable detail and marks
the scan for catch-up instead of allowing an unbounded cache document.
([cache budgets](https://github.com/steipete/CodexBar/blob/50c5b00221ff5bff2f8582c8f6d1f434f428be41/Sources/CodexBarCore/Vendored/CostUsage/CostUsageCache.swift#L3-L20),
[bounded save](https://github.com/steipete/CodexBar/blob/50c5b00221ff5bff2f8582c8f6d1f434f428be41/Sources/CodexBarCore/Vendored/CostUsage/CostUsageCache.swift#L142-L223))

## Pricing and repricing

CodexBar reads model prices from a cached `models.dev` catalog. It refreshes a
stale catalog on a detached utility task. When a scan finds an unpriced model,
it requests a targeted catalog refresh, with a 15-minute retry limit. Unknown
pricing remains unknown; it is not converted to a zero-cost claim.
([pricing refresh and unknown-model retry](https://github.com/steipete/CodexBar/blob/50c5b00221ff5bff2f8582c8f6d1f434f428be41/Sources/CodexBarCore/CostUsageFetcher.swift#L618-L695),
[catalog retry policy](https://github.com/steipete/CodexBar/blob/50c5b00221ff5bff2f8582c8f6d1f434f428be41/Sources/CodexBarCore/Vendored/CostUsage/ModelsDevPricing.swift#L589-L655))

A changed pricing key currently makes the scanner force a bounded full JSONL
scan and rebuild cost rows. The Workspaces sidecar also treats the pricing key
as part of rollout identity. Therefore, CodexBar does not currently prove that
repricing is a source-free operation, even though its normalized event rows
contain enough pricing inputs for a different implementation to do that.
([pricing invalidation](https://github.com/steipete/CodexBar/blob/50c5b00221ff5bff2f8582c8f6d1f434f428be41/Sources/CodexBarCore/Vendored/CostUsage/CostUsageScanner.swift#L4304-L4368),
[full-scan decision](https://github.com/steipete/CodexBar/blob/50c5b00221ff5bff2f8582c8f6d1f434f428be41/Sources/CodexBarCore/Vendored/CostUsage/CostUsageScanner.swift#L4835-L4853))

## User-visible progress

CodexBar preserves the last complete report while catch-up is pending and
returns that report instead of publishing partial totals. In the menu card, it
keeps the cost values and adds a small spinner beside **Cost**. The larger
**Usage & Spend** panel shows byte or file progress, a progress bar, a stale-data
badge when applicable, and **Finish now**, **Continue in background**, and
**Cancel** actions. ([last-good report](https://github.com/steipete/CodexBar/blob/50c5b00221ff5bff2f8582c8f6d1f434f428be41/Sources/CodexBarCore/Vendored/CostUsage/CostUsageScanner.swift#L4401-L4443),
[return last-good until complete](https://github.com/steipete/CodexBar/blob/50c5b00221ff5bff2f8582c8f6d1f434f428be41/Sources/CodexBarCore/Vendored/CostUsage/CostUsageScanner.swift#L4664-L4711),
[compact spinner](https://github.com/steipete/CodexBar/blob/50c5b00221ff5bff2f8582c8f6d1f434f428be41/Sources/CodexBar/MenuCardView.swift#L452-L477),
[detailed progress panel](https://github.com/steipete/CodexBar/blob/50c5b00221ff5bff2f8582c8f6d1f434f428be41/Sources/CodexBar/PreferencesSpendDashboardPane.swift#L161-L231))

On a first-ever scan with no snapshot, the compact cost section is absent
because it requires a snapshot. The detailed panel is the progress surface.
CodexBar describes the result as an estimate from local token usage; the
inspected cost path does not reconcile or scale it against the account quota
total. This last sentence is an inference from the local-scan input and UI
contract, not an explicit repository statement.
([snapshot requirement](https://github.com/steipete/CodexBar/blob/50c5b00221ff5bff2f8582c8f6d1f434f428be41/Sources/CodexBar/MenuCardView%2BCosts.swift#L153-L167),
[estimate label](https://github.com/steipete/CodexBar/blob/50c5b00221ff5bff2f8582c8f6d1f434f428be41/Sources/CodexBarCore/Providers/Codex/CodexProviderDescriptor.swift#L66-L75),
[local scan result](https://github.com/steipete/CodexBar/blob/50c5b00221ff5bff2f8582c8f6d1f434f428be41/Sources/CodexBarCore/CostUsageFetcher.swift#L511-L608))

## Subagent accounting

Codex subagent rollouts can copy the parent history into the child file. The
shared `session_id` is not a safe leaf identity. CodexBar uses the first
`session_meta.payload.id` as the leaf ID, reads several possible parent-ID
fields, and treats the subagent source marker as lineage evidence only.
([leaf identity](https://github.com/steipete/CodexBar/blob/50c5b00221ff5bff2f8582c8f6d1f434f428be41/Sources/CodexBarCore/Vendored/CostUsage/CostUsageScanner.swift#L2627-L2651),
[parent fields](https://github.com/steipete/CodexBar/blob/50c5b00221ff5bff2f8582c8f6d1f434f428be41/Sources/CodexBarCore/Vendored/CostUsage/CostUsageScanner.swift#L2539-L2567),
[lineage warning](https://github.com/steipete/CodexBar/blob/50c5b00221ff5bff2f8582c8f6d1f434f428be41/Sources/CodexBarCore/Vendored/CostUsage/CodexSubagentRolloutShape.swift#L9-L18))

When `subagent_history_start_ordinal` exists, CodexBar ignores records before
that boundary. Otherwise it must resolve the exact parent counter at the fork
and subtract that baseline. If neither boundary is defensible, it does not
publish child token rows. This is separate from ordinary event-row
deduplication.
([explicit boundary](https://github.com/steipete/CodexBar/blob/50c5b00221ff5bff2f8582c8f6d1f434f428be41/Sources/CodexBarCore/Vendored/CostUsage/CostUsageScanner.swift#L3966-L4121),
[parent subtraction](https://github.com/steipete/CodexBar/blob/50c5b00221ff5bff2f8582c8f6d1f434f428be41/Sources/CodexBarCore/Vendored/CostUsage/CostUsageScanner.swift#L3367-L3396),
[unresolved-child rule](https://github.com/steipete/CodexBar/blob/50c5b00221ff5bff2f8582c8f6d1f434f428be41/Sources/CodexBarCore/Vendored/CostUsage/CostUsageScanner.swift#L3438-L3493))

TouchGrassBar uses the same safe boundary rule. Current rollout files that mark
a subagent but do not give a proven history ordinal are excluded from local
cost detail. Their account tokens remain visible through the authoritative
account usage result.

## What TouchGrassBar should copy

1. Run all rollout parsing on one dedicated serial worker.
2. Give the foreground refresh a short bounded pass, save a safe cursor, then
   continue automatically with resource-aware catch-up passes.
3. Validate append resumes with file identity and a hash anchor near the cursor.
4. Keep and show the last complete estimate. Use a small **Indexing…** state in
   the compact card and put byte/file detail and controls in a larger view.
5. Keep account-observed tokens and local API-equivalent cost as separate facts.
   Do not show internal reconciliation labels in the compact UI.
6. Refresh unknown model pricing automatically and log the model for diagnosis.
   Keep unknown cost unknown.
7. Do not copy CodexBar's full source reread for a price-only change. TouchGrassBar
   already has a normalized local index, so it can reprice that index without
   reading rollout content again, if its stored inputs are complete.
