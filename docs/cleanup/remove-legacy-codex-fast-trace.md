# Remove Legacy Codex Fast Trace Compatibility

- **Status:** planned
- **Owner issue:** [#72](https://github.com/FabienGreard/TouchGrassBar/issues/72)
- **Implementation:** [PR #65](https://github.com/FabienGreard/TouchGrassBar/pull/65)

## Scope

The Codex Fast pricing reader accepts one old provider submission format. This
read path is local and bounded. It does not write or change provider data.

The current `response.create` request format is the required replacement.

## Execution plan

1. Use the supported Codex version for one full 30-day retained window.
2. Run a sanitized count-only probe for that window.
3. Confirm that no Fast turn has only the old provider submission as proof.
4. Remove the old SQL candidate, parser branch, reducer support, and tests.
5. Run the Codex usage, pricing, privacy, and bounded-scan tests.
6. Remove this entry in the same cleanup change.

## Verification

Record only the number of old-format rows and old-format-only Fast turns. Both
counts must be zero for the retained 30-day window. The current request-format
tests must still detect Fast and Priority usage.

Do not record trace bodies, turn identifiers, paths, prompts, sessions, or
other provider content.

## Recovery

Restore the bounded old-format read path if the current request path loses Fast
evidence after removal. This cleanup does not change stored data, so it does
not need a data rollback.

## Cleanup targets

- `LEGACY_TARGET`, `LEGACY_SUBMISSION_MARKER`, `LEGACY_SETTINGS_MARKER`, and
  `LEGACY_PRIORITY_MARKER` in `fast_pricing.rs`;
- both legacy SQL candidate filters in `fast_pricing.rs`;
- the legacy provider submission branch in `parse_trace_evidence`;
- reducer support that exists only for model-less legacy Fast evidence;
- legacy Fast trace fixtures and tests; and
- this cleanup entry.

## Exit condition

Issue #72 has count-only evidence that the retained 30-day window has zero
old-format rows and zero old-format-only Fast turns. The current
`response.create` path must pass all Fast and Priority pricing tests.
