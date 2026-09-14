# Verify Automatic Usage History Recovery

- **Status:** rehearsed
- **Owner issue:** [#109](https://github.com/FabienGreard/TouchGrassBar/issues/109)
- **Implementation:** [commit 587851bf](https://github.com/FabienGreard/TouchGrassBar/commit/587851bfd7d5b1228fb3a4036204097b06da0402) (local candidate; not yet published)

## Scope

Verify the automatic recovery of missing original-window days on the first
Active Mac and the Claude parser-12 rescan. Verify that the matching backend
accepts optional dated diagnostic fields before an updated desktop sends them.
The recovery policy and optional protocol fields are permanent. This entry
tracks rollout evidence only. It does not authorize a production deployment.

## Execution plan

1. Use the release workflow to deploy the matching backend before the desktop.
2. Update a first-generation client with incomplete retained history and let
   its bounded scan finish. Do not reset its Profile, index, or sync ledger.
3. Let normal synchronization drain recovered days and retained corrections.
4. Compare selected local daily totals with server daily totals and all public
   windows. Record only counts and equality checks.
5. Refresh again and verify no duplicate usage. Check that a dated parser
   report distinguishes affected records from excluded records when a failure
   occurs; a successful scan is silent.

## Verification

Local rehearsal on 2026-09-14 passed 729 native tests (one ignored), 84 backend
tests, and 22 contract tests. Backend typecheck and lint, Rust clippy, generated
contracts, and formatting passed. Convex accepted the code at the existing
local deployment on port 3210. Synthetic history of 500 million tokens plus a
recovered 1.4 billion-token day produced 1.9 billion-token seven- and thirty-day
scores. Repeated submission was idempotent. A parser-11 excluded file replayed
through parser 12 without duplicate tokens. The native outbox admitted a
revision-one recovered day and removed it after acknowledgement.

Production execution and source-to-server equality checks are pending.

## Recovery

Keep the source files and pending snapshots. Normal refresh resumes scanning
and retries delivery. Stop rollout if generation checks, date limits, exact
retry behavior, or count equality fail. Do not invent daily totals or reset a
Profile to bypass an error. Do not roll back the backend validator while an
updated client can still send the optional diagnostic fields.

## Cleanup targets

Remove this rollout checklist and close its owner issue after verification.
Keep the permanent recovery rules, protocol compatibility, and regression tests.
No database migration, administrative repair mutation, or temporary schema
field is needed.

## Exit condition

The authorized release has an updated first-generation client whose recovered
local daily totals equal the synchronized daily totals and public window sums,
with no duplicate increase on a repeated refresh. The matching backend accepts
both older reports and dated parser reports. Record sanitized release evidence
in the owner issue before removing this entry.
