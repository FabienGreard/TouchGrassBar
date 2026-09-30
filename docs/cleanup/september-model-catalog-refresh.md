# September model catalog rollout

- **Status:** planned
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
rollout evidence is still required.

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
