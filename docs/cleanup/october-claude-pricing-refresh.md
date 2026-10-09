# October Claude pricing rollout

- **Status:** running
- **Owner issue:** [#99](https://github.com/FabienGreard/TouchGrassBar/issues/99)
- **Implementation:** [pricing and analytics fix](https://github.com/FabienGreard/TouchGrassBar/commit/b8655ebaa60846454fd72f532a8af7cf8b7ff363).

## Scope

Catalog `anthropic-standard-2026-10-09-v1` adds Haiku 5.5 prompt-length tiers
from October 7 and the Sonnet 5.5 cache-read change from the same date. Keep
`anthropic-standard-2026-09-30-v1` approved for older clients and unchanged
retained days. This pricing change adds no schema, parser revision, or index
reset. Model detail stays on the Mac; the backend cannot reprice it directly.

The normal local repricing pass uses retained billable counters. Haiku 5.5
detail gains a price only on or after its launch. Sonnet 5.5 cache-read detail
changes only on or after October 7. Other dated prices and all validated token
totals remain. Existing rules still govern modeled estimates, missing paid
metadata, and partial scan coverage.

## Execution plan

1. Run native pricing and retained-detail recovery tests. Verify both prompt
   tiers, every cache category, effective dates, modifiers, stable token totals,
   and an unchanged second scan.
2. Generate the additive native pricing-basis contract. Run backend tests that
   accept both catalogs and preserve Token Score when only cost changes.
3. Deploy the backend approval before the desktop client can send the new
   basis. This step completed before the signed v0.0.59 desktop release.
4. Let an affected updated client finish its normal bounded repricing and
   synchronization. Record count-only local aggregate evidence and accepted
   daily revisions in issue #99. Do not use silence in Failure Reports as proof
   of recovery.
5. Remove the prior basis only when supported clients and the retained 60-day
   history window no longer need it.

## Verification

Local rehearsal on 2026-10-09:

- The three required native regressions failed before the change: Haiku 5.5
  had no price and Sonnet 5.5 still charged USD 0.20 after October 7. All 20
  Claude pricing tests pass after the change. They prove that 100,000 prompt
  tokens use the lower tier. One additional input or cached token selects the
  higher tier for the whole request. Output does not select the tier. Dates,
  modifiers, invalid rate rejection, and unchanged unused-rate fingerprints
  also pass.
- The four new provider-audit regressions failed before the audit change.
  All 37 audit tests pass after the change. They compare the boundary and all
  five higher rates, validate cache-read rules, and reject incomplete or
  inconsistent official row pairs.
- The backend approval regression failed before contract generation because
  the new basis was not approved. It passes with the generated contract. Both
  dated bases synchronize with identical observed tokens and Token Score.
- The retained Haiku recovery test passes in the Claude usage suite. Its
  disposable index keeps 1,200 observed tokens and the scan checkpoint. A
  normal zero-byte scan recovers the priced tokens and cost, adds one daily
  cost revision, and leaves the repeated scan unchanged.
- The retained Sonnet cache-read test also passes. A stored October 9 row
  changes from USD 0.20 to USD 0.10 per million cache-read tokens. The normal
  zero-byte scan keeps 1,000 observed and priced tokens and the checkpoint,
  lowers only cost, adds one daily revision, and leaves the repeat unchanged.

Run the focused checks with:

```sh
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --lib providers::claude::pricing::tests
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --lib catalog_update_reprices_retained_haiku_without_reparsing_or_recounting
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --lib catalog_update_reprices_retained_sonnet_cache_reads_once_without_reparsing
bun scripts/run-vitest.ts scripts/provider-contract-audit.test.ts
bun run --cwd packages/backend test convex/sync.test.ts -t 'a Claude pricing catalog correction'
```

The read-only live provider audit confirms that the new rates and dates match
the current official sources. The full provider review still has source and
evidence drift owned by issue #99. This targeted review keeps the full review
date and broader rule and evidence window hashes unchanged.

Release evidence on 2026-10-09:

- Candidate [a77c62a21238fc72f82d0ceff1e40a1d9e4d3d49](https://github.com/FabienGreard/TouchGrassBar/commit/a77c62a21238fc72f82d0ceff1e40a1d9e4d3d49)
  passed [main CI](https://github.com/FabienGreard/TouchGrassBar/actions/runs/37927242325)
  and the [release workflow](https://github.com/FabienGreard/TouchGrassBar/actions/runs/37927944017).
- Production deployment `next-pig-820` completed before `12:05:42Z`. The
  deployed backend includes the new pricing-basis approval and additive
  diagnostic fields.
- At `12:06:23Z`, the production check verified one successful synchronization,
  four global reads, and four populated rows. It found no failures in the
  checked activity.
- The signed [v0.0.59 release](https://github.com/FabienGreard/TouchGrassBar/releases/tag/v0.0.59)
  became public at `12:20:28Z`. The public update-feed check passed.

These checks prove deployment and publication. They do not prove client
adoption, use of the new pricing basis in a client synchronization, or recovery
on an affected Mac. Record the affected-client evidence before removal.

## Recovery

Keep retained detail, the existing index, and all approved prior bases. An
interrupted repricing pass resumes through the normal scan. Do not reset
history or mark partial coverage complete. If synchronization rejects the new
basis, check the backend contract deployment before retrying the release.

## Cleanup targets

Remove `anthropic-standard-2026-09-30-v1` from
`RETAINED_CLAUDE_PRICING_BASES` in
`apps/desktop/src-tauri/src/providers/mod.rs` only after the exit condition.
Then regenerate `packages/contracts/src/native.generated.ts`. Keep the new
rates, date rules, prompt tiers, and regression tests. Remove this tracker
after the owner issue contains the required evidence.

## Exit condition

The release is public, an affected updated client has
synchronized recovered pricing with unchanged token totals and an unchanged
repeat scan, and no supported client or retained row needs the prior basis.
