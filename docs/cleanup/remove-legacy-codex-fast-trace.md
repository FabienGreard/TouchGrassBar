# Remove Legacy Codex Fast Trace Compatibility

- **Status:** planned
- **Owner issue:** [#72](https://github.com/FabienGreard/TouchGrassBar/issues/72)
- **Implementation:** [PR #65](https://github.com/FabienGreard/TouchGrassBar/pull/65)

## Scope

The Codex Fast pricing reader accepts one old provider submission format. This
read path is local and bounded. It does not write or change provider data.

The current `response.create` request format is the required replacement.

## Execution plan

1. Use the supported Codex version for one full 30-day private Fast
   pricing-detail window.
2. Run a sanitized count-only probe for that private detail window.
3. Confirm that no Fast turn has only the old provider submission as proof.
4. Remove the old SQL candidate, parser branch, reducer support, and tests.
5. Run the Codex usage, pricing, privacy, and bounded-scan tests.
6. Remove this entry in the same cleanup change.

## Verification

Record only the number of old-format rows and old-format-only Fast turns. Both
counts must be zero for the private 30-day pricing-detail window. The current
request-format tests must still detect Fast and Priority usage. Sanitized daily
token aggregates and parser deduplication metadata have a separate 60-day
retention window and are not cleanup evidence for this private trace format.

Do not record trace bodies, turn identifiers, paths, prompts, sessions, or
other provider content.

## Recovery

Restore the bounded old-format read path if the current request path loses Fast
evidence after removal. This cleanup does not change stored data, so it does
not need a data rollback.

## Cleanup targets

- `LEGACY_TARGET`, `LEGACY_SUBMISSION_MARKER`, `LEGACY_SETTINGS_MARKER`, and
  `LEGACY_PRIORITY_MARKER` in `fast_pricing.rs`;
- the `legacy_filter` branches in `load_fast_turns_from_database` and
  `load_fast_turns_full_scan`;
- the `LEGACY_TARGET` fallback branch in `parse_trace_evidence`;
- the `production_shaped_legacy_submission_proves_fast_without_a_model` and
  `legacy_submission_uses_the_separate_trusted_target_column` tests;
- the legacy assertion in `malformed_provider_evidence_fails_closed`; and
- `docs/cleanup/remove-legacy-codex-fast-trace.md`.

## Exit condition

Issue #72 has count-only evidence that the private 30-day pricing-detail window
has zero old-format rows and zero old-format-only Fast turns. The current
`response.create` path must pass all Fast and Priority pricing tests.
