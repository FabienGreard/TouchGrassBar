# November model catalog rollout

- **Status:** blocked
- **Owner issue:** [#99](https://github.com/FabienGreard/TouchGrassBar/issues/99)
- **Implementation:** [November pricing catalogs](https://github.com/FabienGreard/TouchGrassBar/commit/dd456226ed42e79cb5bbf7ab481c7effe491232c)
  and [GPT-6.2 pricing](https://github.com/FabienGreard/TouchGrassBar/commit/0fa120456a1f360fea47c956a7828a015e60627d).

## Scope

Add GPT-6.1 Terra, GPT-6.2 Astra, GPT-6.2 Sol, Claude Fable 5.2, and Claude
Mythos 5.2 to local API-Equivalent Cost. The new bases are
`openai-standard-2026-11-18-v1` and
`anthropic-standard-2026-11-13-v1`. Keep `openai-standard-2026-09-30-v1` and
`anthropic-standard-2026-10-09-v1` approved for older clients and retained
rows. The normal scan recalculates cost from retained token counts.
Codex parser 26 also reads the current task and token-count metadata. The
normal scan replays retained Codex logs when a stored checkpoint uses an
earlier parser.

## Execution plan

1. Run the native price and recovery tests and the Usage Snapshot sync suite.
2. Deploy the additive pricing bases and verify production sync and reads.
3. Publish the desktop release and verify the stable update feed.
4. In issue #99, record aggregate evidence from an updated client that
   synchronized recovered pricing. Include priced token coverage and confirm
   that observed token totals stay the same.
5. Remove the prior bases after supported clients and retained 60-day history
   no longer use them.

## Verification

Local rehearsal on 2026-11-13 passed 808 native library tests, including the
launch prices and retained-cost recovery. The recovery tests verified the
parse checkpoint, observed token totals, recovered cost, and a repeated
repricing pass. All 55 Usage Snapshot sync tests passed, including sync with
each approved pricing basis.

The live audit on 2026-11-18 compared the new models with current provider
prices and launch dates. Issue #99 tracks the remaining provider contract
findings.

On 2026-11-18, candidate
[`c8a9a22a`](https://github.com/FabienGreard/TouchGrassBar/commit/c8a9a22a540e0d4cb1620491d8d73a7efcfba97e)
passed [main CI](https://github.com/FabienGreard/TouchGrassBar/actions/runs/44284635073).
The dry deployment and production deployment to `next-pig-820` passed.
The production check found a successful sync and 10 leaderboard rows,
including three with cost.

Apple rejected [the v0.0.63 build](https://github.com/FabienGreard/TouchGrassBar/actions/runs/44287614196)
at notarization because an agreement is missing or has expired. The account
holder must resolve the agreement before the next candidate can pass
notarization. The next candidate is v0.0.64. Record released-client recovery
evidence in the owner issue.

## Recovery

Keep retained detail, the local index, and the approved prior bases. An
interrupted repricing pass resumes during the normal scan. If synchronization
rejects a new basis, verify the backend deployment before retrying.

## Cleanup targets

Remove `openai-standard-2026-09-30-v1` and
`anthropic-standard-2026-10-09-v1` from the retained basis lists in
`apps/desktop/src-tauri/src/providers/mod.rs`. Regenerate
`packages/contracts/src/native.generated.ts` and remove this entry.

## Exit condition

The release is public, issue #99 contains client recovery evidence, and no
supported client or retained row needs either prior basis.
