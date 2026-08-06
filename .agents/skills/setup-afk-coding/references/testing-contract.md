# Minimum Sufficient Proof

Use ticket-shaped proof. Derive required behavior only from ticket acceptance criteria, a confirmed bug reproduction, a named ADR or domain invariant, or an explicit privacy, authorization, destructive-action, migration, concurrency, persistence, or compatibility boundary. A changed file, package, language, or framework does not select a test or gate by itself.

The default budget is zero new tests and one focused local proof. Add evidence only for a distinct named failure that the current budget cannot catch. `Review only` and `No test change required` are valid results.

## 1. Map required behavior

Read the ticket and the tests nearest to the changed public seam. Group acceptance criteria that describe the same observable behavior. For each behavior, identify its source, the failure that matters, existing evidence, and the smallest proof that can catch that failure. Stop when all required behavior is mapped; leave the rest of the test tree unexplored.

Treat framework and library behavior as established unless the application changes it. Treat a focused test that exercises several criteria as one proof.

Completion criterion: every required behavior maps to one named failure and one proof; no proof exists only because a file, package, language, or framework changed.

## 2. Publish the Proof Budget

Before editing, publish this compact table in a Codex commentary update. Copy it into the draft PR.

```markdown
## Proof Budget

| Required behavior | Source | Failure | Smallest proof |
| --- | --- | --- | --- |
| <behavior> | <issue AC, reproduction, ADR, or invariant> | <observable regression> | <existing test and command, new test and command, or review only> |

New tests: <0 by default>
Extra local gates: <none, or command — distinct failure caught>
```

One proof can cover several rows. Use review-only proof when the diff directly proves the requirement, such as documentation, copy, a static value, safe deletion, or declarative configuration. Add a test only when it:

1. has authoritative provenance;
2. fails for the named regression;
3. uses the nearest stable public seam;
4. catches a failure that no budgeted proof catches; and
5. is the smallest proof for that failure.

Completion criterion: every command catches a named failure, and no two commands prove the same failure.

## 3. Match evidence to failure

Select the failure first. Then select the narrowest evidence:

- For a runtime behavior regression, use one focused behavioral test at the nearest stable public seam.
- For a type, compile, build, or generated-contract failure that the focused proof does not exercise, use the narrowest command that reaches that path.
- For a changed serialization or native boundary, use one contract proof at that boundary. Add proof on both sides only when each side can fail independently.
- For a changed authorization or privacy rule, use the public backend seam and cover the allowed and refused outcomes that the rule names.
- For a changed migration, destructive action, persistence, concurrency, or compatibility rule, use one focused special proof for that rule.
- For a required pixel, device, or physical outcome, use the smallest observable evidence and name any human verification boundary.
- For documentation, workflow, or configuration, use a focused parser or validation command when malformed structure is the named risk; otherwise use review-only proof.

Language and package determine how to run selected evidence. They do not increase the evidence count.

Completion criterion: the budget contains the smallest set of independent proofs that can catch every named failure.

## 4. Execute the bounded proof

1. When the budget adds or changes a test, prove it red for the named reason before implementation and green afterward.
2. Run each focused proof once after implementation.
3. Run an extra local gate only when its Proof Budget entry names a failure that the focused proof cannot catch.
4. Push the exact verified head and use required hosted checks as repository-wide integration proof.

The required hosted check is the normal full-suite gate. Use local root `bun quality` only to diagnose a hosted failure or when hosted checks cannot run for the PR.

Retry a suspected flaky command once without code changes. When a failure reproduces on the clean base or lies outside the owned change, attach comparative evidence, add `agent:blocked` to both the issue and draft PR, and stop.

Completion criterion: every budgeted command and required hosted check has one terminal result tied to the exact PR head.

## 5. Stop at sufficiency

Stop when every Proof Budget row has sufficient evidence. Leave speculative edge cases without authoritative provenance outside the worker output.

Before handoff, remove worker-authored tests that lack provenance, duplicate a budgeted failure, assert private implementation details, mock the subject under test, or only increase coverage counts. Retain a snapshot change only when the ticket requires that visual result.

Completion criterion: every added or changed test traces to one Proof Budget row, and every Proof Budget row is necessary for a distinct required failure.
