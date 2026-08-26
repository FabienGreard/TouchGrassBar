# Bundled API pricing

`openai-standard.json` and `anthropic-standard.json` are the offline price
sources for API-equivalent cost. The application embeds these files at compile
time. It does not download price updates.

To update prices, edit the manifest, change its `basis`, test the effective
date ranges, and release a new application version. On the first run of that
version, TouchGrassBar reprices retained private SQLite cost details. It does
not parse provider content again for a price-only change.

A live pricing page proves the current rate but does not always prove when that
rate began. Each new price period requires a dated first-party release note,
announcement, or change record for its inclusive start. Keep the current price
page as supporting evidence. A statement that a rate is available at least
through a date is not an exclusive end date. Leave the period open until a
first-party source publishes the next effective change.

The index stores a semantic fingerprint of the validated manifest to detect an
update even if the basis was not changed by mistake. Each private priced detail
also stores the fingerprint of its applicable pricing rule. A manifest update
changes only aggregates whose applicable rule changed. The basis remains the
readable price-book version that the application can show in sanitized output.

The Rust parsers reject invalid manifests. They also reject unknown billing
fields or mark the related usage partial and unpriced. Keep each format suitable
for a future signed remote manifest. Do not add remote updates as part of the
local usage work.

## OpenAI rules

When the local debug report finds an unknown model, check the official OpenAI
API pricing page, model catalog, Codex rate card, and official Codex source.
Add a manifest entry only when those sources define every applicable input,
cached-input, cache-write, output, effective-date, and long-context rule. Add
an alias only when an official source defines it.

If any required price or alias is missing, leave the model out of the manifest.
That model's local tokens stay unpriced. A period can still show a modeled best
estimate when other priced local evidence supplies a defensible rate; its
coverage reports how much local detail was priced. If the period has no usable
priced evidence, API-equivalent cost stays unavailable while account Observed
Tokens remain visible. Updating the manifest and releasing the application are
manual operations.

The Codex scanner uses Standard prices unless an exact `response.create`
request proves that a matching turn used the `priority` or `fast` service
tier. A trusted provider submission can also prove this tier. A completed
response can refine the request model. For short context, the
scanner uses `fastMultiplier` when the selected model supports it. For requests
with more than 272,000 input tokens, GPT-5.6 Sol, Terra, and Luna use the dated
`fastLongContext` rates from August 5, 2026. GPT-5.5 has no published Fast
long-context rates. Its Fast long-context cost stays unpriced.

This rule intentionally differs from the current CodexBar implementation.
CodexBar uses Standard long-context rates as a fallback for Fast requests.
The official OpenAI pricing page and API changelog take precedence over that
fallback. The scanner stores only the private turn classification in its local
index. No turn identifier enters the sanitized state or sync payload.

A stable fingerprint of the retained Fast evidence invalidates old local
classifications when trace evidence arrives after a rollout was indexed. The
scanner then rebuilds the private retained usage index. Regional processing
also stays Standard unless a future reviewed local source reports the exact
region. Do not infer a regional uplift from the user location or account.

The audit also snapshots the bounded OpenAI token and tool pricing region. A
tool-price change requires review even though the token manifest has no
tool-rate fields.

Use these primary sources for an OpenAI update:

- [OpenAI API pricing](https://developers.openai.com/api/docs/pricing)
- [OpenAI Fast mode](https://developers.openai.com/api/docs/guides/fast-mode)
- [OpenAI API changelog](https://developers.openai.com/api/docs/changelog)
- [OpenAI model catalog](https://developers.openai.com/api/docs/models)
- [GPT-5.6 Sol](https://developers.openai.com/api/docs/models/gpt-5.6-sol)

The `2026-08-26-v1` review used these dated first-party changes:

- OpenAI changed GPT-5.6 Terra and Luna rates on
  [2026-07-30](https://openai.com/index/advancing-the-price-performance-frontier-with-gpt-5-6/).
  Terra changed to USD 2 input, USD 0.20 cached input, USD 2.50 cache
  write, and USD 12 output per million tokens. Luna changed to USD 0.20,
  USD 0.02, USD 0.25, and USD 1.20.
- OpenAI changed GPT-5.6 Sol rates on
  [2026-08-21](https://openai.com/index/gpt-5-6/). Sol changed to USD 4
  input, USD 0.40 cached input, USD 5 cache write, and USD 20 output per
  million tokens.
- The [current OpenAI price table](https://developers.openai.com/api/docs/pricing)
  confirms these Standard and Fast short-context and long-context rates.
  OpenAI states that GPT-5.6 Fast mode costs two times its Standard rate. The
  Sol offer is valid at least through 2026-11-21. That date is not an exclusive
  end date.

## Anthropic rules

`anthropic-standard.json` contains public Claude API list prices. It does not
contain private offers, volume discounts, partner cloud prices, or Priority
Tier commitments. The result is an API-equivalent estimate, not an invoice.

Each price period has an inclusive start date and an exclusive end date. The
manifest stores separate rates for input, 5-minute cache writes, 1-hour cache
writes, cache reads, and output. The Rust parser checks the published cache
factors. It also checks that date ranges do not overlap and that each fast-mode
period is inside a standard price period.

The pricing code applies these supported modifiers:

- Batch uses 0.5 times the token rates.
- US inference uses 1.1 times the token rates for supported models.
- Fast mode uses its dated rate for a supported model.
- Web search adds USD 10 for 1,000 successful searches.
- Web fetch has no charge in addition to its token cost.

The factors stack, except that fast mode and Batch cannot be used together.
The reviewed usage schema does not include a code-execution counter. The
scanner detects a code-execution server-tool block from its bounded type and
name metadata. The pricing code omits cost for that block or for a future
nonzero code-execution count because neither contains the time-based charge.
The absence of both does not block token pricing. Missing or unknown paid
metadata also omits cost.

Do not add a model, alias, price, modifier, or effective date without an
official Anthropic source. Do not apply this manifest to Amazon Bedrock or
Google Cloud model IDs. Unknown models and provider-specific prices remain
unpriced.

Use these primary sources for a Claude update:

- [Claude pricing](https://platform.claude.com/docs/en/about-claude/pricing)
- [Claude model IDs and lifecycle](https://platform.claude.com/docs/en/about-claude/model-deprecations)
- [Claude release notes](https://platform.claude.com/docs/en/release-notes/overview)
- [Anthropic TypeScript SDK usage types](https://github.com/anthropics/anthropic-sdk-typescript/blob/3b45cd3b69c956ac63384fdb09ce1d8109f3fa80/src/resources/beta/messages/messages.ts#L4053-L4105)

The Claude transcript allow-list has three verified pairs. Claude Code
`2.1.223` pairs with `@anthropic-ai/claude-agent-sdk` `0.3.223`. Claude Code
`2.1.224` pairs with SDK version `0.3.224`. Claude Code `2.1.241` pairs with SDK
version `0.3.241`.

The scanner ignores one synthetic API-error record from Claude Code `2.1.241`.
The record must match the reviewed shape exactly. This check includes the
wrapper, message, content, zero-token counters, zero paid-tool counters, and
null extended usage fields. A different API-error shape fails closed. A
different transcript version stays partial or unavailable until controlled
fixtures prove that schema.

The `2026-08-26-v1` review removed the planned Sonnet 5 increase. Anthropic
[canceled it on 2026-08-10](https://platform.claude.com/docs/en/release-notes/overview).
The Sonnet 5 period that began on 2026-06-30 remains open. These rates apply to
one million tokens:

| Usage                |   USD |
| -------------------- | ----: |
| Input                |  2.00 |
| 5-minute cache write |  2.50 |
| 1-hour cache write   |  4.00 |
| Cache read           |  0.20 |
| Output               | 10.00 |

### Claude parser fixture review

The parser treats three nested usage objects as opaque. They are
`fallback_credit`, `iterations`, and `output_tokens_details`. A non-null value
keeps known top-level token counts, but it makes the record partial and
unpriced. Review each full nested type graph and its billing effect before you
remove this guard.

Use this process when the audit reports a new Claude Code or Agent SDK version:

1. Record the exact package version and `dist.integrity` value from the npm
   version record. Do this for Claude Code and the Agent SDK. Do not review a
   moving `latest` or `stable` tag as if it were an exact package.

   ```sh
   curl --fail --silent --show-error \
     "https://registry.npmjs.org/@anthropic-ai%2fclaude-code/${claude_code_version}" \
     | jq -er --arg version "$claude_code_version" \
       'select(.version == $version) | {version, integrity: .dist.integrity}'
   curl --fail --silent --show-error \
     "https://registry.npmjs.org/@anthropic-ai%2fclaude-agent-sdk/${claude_agent_sdk_version}" \
     | jq -er --arg version "$claude_agent_sdk_version" \
       'select(.version == $version) | {version, integrity: .dist.integrity}'
   ```

2. Use a dedicated macOS test account and a new temporary directory. Set
   `CLAUDE_CONFIG_DIR` to that directory before the test. Use a synthetic
   project with no user files or private prompt text.

   ```sh
   claude_fixture_dir="$(mktemp -d)"
   export CLAUDE_CONFIG_DIR="$claude_fixture_dir"
   ```

3. Run the exact Claude Code package. Make a short request and a repeated
   request. The repeated request can create and read a prompt cache. Make an
   extended-thinking request, an available server-tool request, and an aborted
   turn. Keep the raw JSONL only in the temporary directory.

   ```sh
   bunx "@anthropic-ai/claude-code@${claude_code_version}"
   ```

4. Export structure, not values, from each assistant record. This `jq` filter
   keeps the version plus usage key and value types. It omits content, IDs,
   paths, timestamps, models, and counter values. Set `claude_fixture_file` to
   the exact JSONL file inside the temporary config directory:

   ```sh
   jq -c '
     def shape:
       if type == "object" then with_entries(.value |= shape)
       elif type == "array" then {type: "array", items: ([.[] | shape] | unique)}
       else type
       end;
     select(.type == "assistant")
     | {version, usage: (.message.usage | shape)}
   ' "$claude_fixture_file"
   ```

5. Compare the structural export with `RawClaudeTokenUsage`, the reviewed field
   signatures, and the token-meaning markers. Classify every new field as an
   inclusive total, additive counter, pricing modifier, paid-tool count, or
   ignored metadata. An unclassified field must stay partial and unpriced.
6. Build a minimal synthetic Rust fixture. Replace all raw values. Test normal,
   missing, null, unknown, oversized, and aborted cases. Confirm that unknown
   counters cannot enter debug output or SQLite.
7. On macOS, run the Claude usage tests and the provider audit:

   ```sh
   cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml \
     providers::claude::usage::tests
   bun run audit:providers -- --format markdown
   ```

8. Add the exact Claude Code version to the allow-list only after every fixture
   passes. Update the reviewed package versions, signatures, semantic markers,
   and full-review date as applicable. Delete the temporary config and raw
   JSONL. Never commit or attach them.

## Provider contract audit

Run the public-source audit with this command:

```sh
bun run audit:providers -- --format markdown
```

The audit checks the documented Codex and Claude usage contracts and pricing
sources. It does not update a parser, fixture, dependency, or pricing manifest.
For OpenAI, it compares the Standard and Fast tables and checks the reviewed
Fast-mode and regional-processing statements. Bounded rule-window hashes cover
the related token and tool pricing sections. An added surcharge or modifier
requires review.

The audit binds each reviewed pricing change to dated first-party changelog or
release-note evidence. Every manifest start and end must have a section
checkpoint or an explicit legacy exemption. The audit hashes each dated
section and the bounded release-note window. A later correction requires review
even when the old statement remains.

The audit reports one of these states:

- `pass`: The available public evidence matches the reviewed contract.
- `review-required`: Public evidence changed or no longer proves part of the
  reviewed contract. A maintainer must review the affected parser, fixtures,
  token categories, aliases, effective dates, and prices.
- `unavailable`: The audit could not retrieve or interpret all required public
  evidence. This state is not a pass. A maintainer must check the source and
  run the audit again.

The command exits with code `0` for `pass`, `2` for `review-required`, and `3`
for `unavailable`.

All contract and price changes require manual review. Do not make an automatic
parser or pricing change from this report. Confirm each change with an official
provider source. Add controlled fixtures. Run the applicable parser and pricing
tests before release.

After the review is complete, update only the affected values and semantic
hashes in `provider-contracts/reviewed.json`. Advance `reviewedAt` only after
one review checks every Codex and Claude parser and pricing area. A price-only
or provider-only review must keep the existing date. Do not change the snapshot
only to clear an issue. Run the audit again and confirm that no accepted change
remains in the report.

The Markdown report is safe for a GitHub issue and workflow summary. It must
contain only public contract evidence and sanitized audit details. It must not
contain provider credentials, authorization headers, account data, local file
paths, transcript content, or raw provider responses. The workflow publishes
only the Markdown standard output. It does not publish diagnostic standard
error and does not receive provider credentials.

The `Provider contract audit` GitHub Actions workflow runs each week and can
also run manually. A `review-required` or `unavailable` result creates or
updates the open `Provider contract audit needs review` issue and applies the
`needs-triage` label. A later `pass` result closes that generated issue.
