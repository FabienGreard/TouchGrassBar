# September model catalog rollout

- **Status:** running
- **Owner issue:** [#99](https://github.com/FabienGreard/TouchGrassBar/issues/99)
- **Implementation:** [49e2eecd](https://github.com/FabienGreard/TouchGrassBar/commit/49e2eecd0d08ffef0f7c5b4c703df9cf2f2e46df)
  updates the bundled pricing manifests and generated native contract.

## Scope

Add GPT-6.1 Sol and Claude Sonnet 5.5 to local API-Equivalent Cost and model
display. The new bases are `openai-standard-2026-09-30-v1` and
`anthropic-standard-2026-09-30-v1`. Keep `openai-standard-2026-09-24-v1` and
`anthropic-standard-2026-09-23-v2` approved for older clients and retained rows.
This change adds no schema, parser version, or index reset.

## Execution plan

1. Deploy the additive approved bases through an authorized backend release.
2. Ship the desktop update through the release workflow.
3. Let normal scans reprice retained details. Record sanitized model counts,
   priced-token coverage, and unchanged token totals in the owner issue.
4. Review the remaining provider audit findings in issue #99. The full review
   date and broader pricing window hashes stay unchanged in this model update.
5. Remove the two retained bases only when supported clients and the 60-day
   history window no longer require them.

## Verification

Price tests prove the inclusive launch dates, cache categories, context bands,
and supported modifiers. Recovery tests remove the synthetic provider source
before repricing. The retained detail gains a cost without a new parse or a
token change. Repeated repricing makes no further aggregate change. The sync
suite accepts every native approved basis, including both new bases. Live
recovery evidence from a released client is still required.

## Release verification

[v0.0.56](https://github.com/FabienGreard/TouchGrassBar/releases/tag/v0.0.56)
is public.
Candidate commit `1e6218fe52fcc3c403d26e2b1c002f920b118e6c` passed
[CI run 36685493492](https://github.com/FabienGreard/TouchGrassBar/actions/runs/36685493492).
The exact backend passed a dry deployment and deployed to production
`next-pig-820` before tag creation. The post-deployment check recorded five
successful usage synchronizations, thirteen successful leaderboard reads, and
no failed executions. A separate bounded read returned 45 populated rows,
including 29 rows with cost.

[Release run 36686349026](https://github.com/FabienGreard/TouchGrassBar/actions/runs/36686349026)
passed all 53 database fixtures, signing, notarization, Gatekeeper, and updater
signature checks. All seven public downloads match the verified draft hashes.
The downloaded updater archive passed a separate signature check. The downloaded
DMG passed separate code signature, notarization ticket, and Gatekeeper checks.
The stable update feed names version 0.0.56 and its signed archive.

These production checks prove backend availability and synchronization from
existing clients. Keep this entry until the released app provides live model
recovery evidence and the retained pricing bases meet the exit condition.

## Recovery

Keep the local index and all approved old bases. An interrupted scan resumes
through the existing bounded scan and repricing paths. Do not reset history.
Deploy the backend approval before a client can send a new basis.

## Cleanup targets

Remove `openai-standard-2026-09-24-v1` and
`anthropic-standard-2026-09-23-v2` from the retained basis lists in
`apps/desktop/src-tauri/src/providers/mod.rs`, then regenerate
`packages/contracts/src/native.generated.ts`. Keep the new model rules, tests,
and launch evidence. Remove this file after the exit condition is true.

## Exit condition

The owner issue contains live recovery evidence, every supported client accepts
the new bases, and no supported client or retained row needs the two old bases.
