# Minimum Sufficient Proof

Use a closed-world test scope. Derive proof only from ticket acceptance criteria, a confirmed bug reproduction, a named ADR/domain invariant, or an explicit privacy, authorization, destructive-action, migration, concurrency, persistence, or compatibility boundary. Record other plausible behavior as follow-up work instead of expanding this PR.

The default budget is zero new tests. Add a test only when existing evidence is insufficient and the proposed test:

1. names its requirement or invariant provenance;
2. would fail when that behavior regresses;
3. exercises the nearest stable public seam;
4. catches a distinct failure not already covered;
5. is the smallest sufficient proof.

`No test change required` is a valid and preferred result when existing proof or the type system is sufficient.

## 1. Inspect before proposing

Search the changed public seam and its directly related tests, fixtures, helpers, type-level guarantees, and CI checks. Run or read the narrow existing proof when its relevance is uncertain. Stop searching as soon as every acceptance criterion is mapped; leave unrelated test-tree inventory outside this PR. Treat framework/library behavior as established unless the application wraps or changes it.

Completion criterion: every acceptance criterion is mapped to existing proof, a specific proof gap, or non-test evidence; no criterion is mapped twice without two distinct risks.

## 2. Publish the Proof Budget

Before editing code or tests, publish this table in a Codex commentary update and later copy it into the draft PR:

```markdown
## Proof Budget

| Requirement or invariant | Provenance | Existing proof | New proof | Command |
| --- | --- | --- | --- | --- |
| AC-1 | issue AC-1 | `<test>` | none | `<focused command>` |
| AC-2 | issue AC-2 | none | one behavioral test | `<focused command>` |

Maximum new tests: <count, initially zero>
Affected-package gate: <command or none>
Special gates: <visual/native-contract/Convex/Rust/none>
Explicitly excluded: <irrelevant suites>
```

Set `Maximum new tests` to the number of distinct uncovered behaviors, not the number of examples or permutations. Reduce the budget when existing proof is discovered. Increase it only when newly found authoritative requirements add distinct uncovered behavior; cite that source in the table before writing the test.

Completion criterion: every planned test has one provenance row and every excluded suite is named.

## 3. Select the narrowest evidence

Use one primary proof per distinct behavior:

| Changed surface | Local evidence |
| --- | --- |
| Desktop TypeScript logic | Focused Vitest file/test plus desktop typecheck |
| Visible desktop UI | Semantic component proof; visual regression only for intentional pixel changes |
| Rust core | Focused Cargo module/test; final fmt/clippy/test only when Rust changed |
| Native TypeScript/Rust boundary | Contract check plus focused proof on each changed side |
| Convex backend | Focused `convex-test`, backend typecheck, and only provenance-backed negative authorization/privacy cases |
| Landing | Astro check/build; desktop visual and Rust suites are outside this surface |
| Docs or workflow only | Relevant syntax/config validation; application suites require a changed application path |

Prefer one parameterized test only when each row is an authoritative required case. Prefer a stronger existing integration proof over duplicating the same risk at unit and end-to-end layers. When a new proof fully supersedes an older proof in the same owned seam, remove the redundant proof and explain the replacement; leave unrelated redundancy as follow-up work.

Completion criterion: each distinct risk has exactly one primary layer unless the Proof Budget explains why separate layers catch separate failures.

## 4. Execute a bounded ladder

1. When the Proof Budget adds or changes a test, prove it red for the intended reason before implementation and green afterward. With zero test changes, run only the narrow existing proof named in the budget.
2. Run the affected-package test/typecheck gate once after implementation when the budget names one.
3. Run only special gates named in the Proof Budget.
4. Push the exact head and use hosted CI as repository-wide integration proof.

Run local root `bun quality` only for cross-package, tooling, contract-generation, or release-sensitive changes, or when hosted CI cannot provide exact-head proof. The hosted `Bun and Rust quality` check is the normal full-suite gate for localized work.

Retry a suspected flaky command once without code changes. When the failure reproduces on the clean base or lies outside the owned change, attach comparative evidence, add `agent:blocked` to both the issue and draft PR, and stop rather than modifying unrelated code or tests.

Completion criterion: every Proof Budget command has one terminal result, and repository-wide proof is tied to the exact PR head.

## 5. Stop at sufficiency

Stop creating, expanding, parameterizing, or strengthening tests as soon as every Proof Budget row has sufficient evidence. Record newly imagined edge cases as follow-up suggestions with their missing provenance.

Before handoff, remove tests added by this worker that lack provenance, duplicate an existing risk, assert private implementation details, mock the subject under test, or only increase coverage counts. Inspect every intentional snapshot diff; retain it only when the ticket requires the visual change.

Completion criterion: every added or changed test traces to one Proof Budget row, and no worker-authored test remains outside the approved budget.
