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

The `2026-09-05-v1` review adds `gpt-6-astra` from its
[September 3 launch](https://developers.openai.com/api/docs/changelog#sep-3).
The [model page](https://developers.openai.com/api/docs/models/gpt-6-astra)
and [price table](https://developers.openai.com/api/docs/pricing) define these
rates per million tokens. The periods start on 2026-09-03 and remain open.

| Usage       | Standard, up to 272K input | Standard, above 272K input | Fast, up to 272K input | Fast, above 272K input |
| ----------- | -------------------------: | -------------------------: | ---------------------: | ---------------------: |
| Input       |                     $10.00 |                     $20.00 |                 $20.00 |                 $40.00 |
| Cache read  |                      $1.00 |                      $2.00 |                  $2.00 |                  $4.00 |
| Cache write |                     $12.50 |                     $25.00 |                 $25.00 |                 $50.00 |
| Output      |                     $50.00 |                     $75.00 |                $100.00 |                $150.00 |

The full request uses the long-context rate when input exceeds 272,000 tokens.
The model has no additional reviewed alias. Fast mode is unavailable with EU
data residency. The audit checks that restriction. These are public Codex
usage estimates; this change does not add regional endpoint billing.

The same review checks Codex `0.153.4` against its
[tagged protocol source](https://github.com/openai/codex/blob/rust-v0.153.4/codex-rs/protocol/src/protocol.rs).
The account and thread usage schemas and API token mapping match the earlier
snapshot. Two new authentication-recovery events have no token counters. The
new optional `forked_from_ordinal_exclusive` field identifies the logical fork
boundary; the scanner retains its existing explicit-boundary and parent
resolution checks. Synthetic root and child fixtures cover CLI minors 152 and
153, contiguous ordinals, ignored recovery events, and repeated indexing.
Parser 20 reparses earlier rejected files and promotes only complete, safe
parser-18 and parser-19 rows.

When the local debug report finds an unknown model, check the official OpenAI
API pricing page, model catalog, Codex rate card, and official Codex source.
Add a manifest entry only when those sources define every applicable input,
cached-input, cache-write, output, effective-date, and long-context rule. Add
an alias only when an official source defines it.

An unqualified official price row can cover every context size. The audit may
use its short-context cells for the long-context comparison only when all four
long-context cells are `-` and the manifest input and output multipliers are
both `1.0`. A row that names a context band stays qualified to that band. A
missing long-context price in such a row fails closed.

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

The `2026-08-26-v2` review used these dated first-party changes:

- OpenAI released
  [GPT-5.2 on 2025-12-11](https://developers.openai.com/api/docs/changelog).
  The current price table lists USD 1.75 input, USD 0.175 cached input, no
  cache-write price, and USD 14 output per million tokens. Its unqualified row
  has no separate long-context rates, so the same token rates apply above the
  272,000-token threshold.
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

Cache writes are always 1.25 times base input for 5 minutes and 2 times base
input for 1 hour. Anthropic prices a cache read at 0.1 times base input, with a
published 0.025 times exception. A period that uses the exception must declare
`cacheReadMultiplier`. Omitting the field means the 0.1 times rule. The Rust
parser rejects any other multiplier and rejects a cache read rate that does not
match the multiplier the period declares. The audit compares the declared set
against the models the pricing page names in that exception. Do not add a third
multiplier without an official Anthropic source.

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

The earlier Claude Code reviews checked four pairs. Claude Code
`2.1.223` pairs with `@anthropic-ai/claude-agent-sdk` `0.3.223`. Claude Code
`2.1.224` pairs with SDK version `0.3.224`. Claude Code `2.1.241` pairs with SDK
version `0.3.241`. Claude Code `2.1.258` pairs with SDK version `0.3.258`.

The 2026-09-05 review adds Claude Code `2.1.236`, `2.1.259`, `2.1.260`, and
`2.1.261`. The current channel matrix is:

| Channel | Claude Code | Agent SDK |
| ------- | ----------- | --------- |
| Stable  | `2.1.236`   | `0.3.236` |
| Latest  | `2.1.261`   | `0.3.261` |

Exact npm version records supply these integrity values:

| Package                          | Version   | `dist.integrity`                                                                                  |
| -------------------------------- | --------- | ------------------------------------------------------------------------------------------------- |
| `@anthropic-ai/claude-code`      | `2.1.236` | `sha512-sz+7GLMhFcwkN2tZHJIXGgon/g/29WMMV5UNYog9sl4OvdX5q3evM1mcXVQnasP4obP6ueItECMCpSk1MPhTDg==` |
| `@anthropic-ai/claude-code`      | `2.1.259` | `sha512-kzhz+R36GgL5aouAkeMO9nI1BEIVaRx1NGu0wTTn/H315l61uiLRo13yvva7H10Pfv0PGgzqJ4m+EKv9BzIRXQ==` |
| `@anthropic-ai/claude-code`      | `2.1.260` | `sha512-Arqg8BvlOehmC3QdACN2WKshqqWQVMo+5NwG22aiJbw7M6S1LM7E2pA2MjD8BS5P5EwZVkh2eKUmC6k7pVUqSQ==` |
| `@anthropic-ai/claude-code`      | `2.1.261` | `sha512-j6+AkfCl6/UJBcx66nlZUmWc4XGK3TscvW19Tiat+oDwkz3WqQfKzjvHO5FhR+shXTtktqs6vqSBrJmeSWpU3Q==` |
| `@anthropic-ai/claude-agent-sdk` | `0.3.236` | `sha512-6SX3gLk4z4cOuixRRILC3QPcVxudJJU6oWm142PFnPADpXS0wYAukGcLueox9AoTZryuGw/JDa9h0yXBOcF8iQ==` |
| `@anthropic-ai/claude-agent-sdk` | `0.3.261` | `sha512-CDG9z14JVKYRHjpp/g6zJ2k8xM5uSoRgjGdpTiK9woLDZxXtXcxV93ipCh55jQ3REj7M7H3GieMsETGZXB/ydw==` |

Each exact Code package ran in `--bare` mode with an isolated
`CLAUDE_CONFIG_DIR`, no tools or MCP servers, a synthetic API key, and a
localhost Messages stream. This checks transcript serialization without an
account request. It does not prove live provider responses. The stable package
omits `apiBlockIndex`; the three later packages include it. Their usage fields
match the reviewed shape, including cache buckets, one matching iteration,
and inclusive thinking-token detail. The Rust fixtures replace all values
and test duplicate records, repeated scans, aborted records, unknown counters,
and invalid breakdowns. Parser 10 reparses retained records to restore complete
coverage and cost for these versions.

The normal usage debug command also read the generated `2.1.261` transcript:
100 observed tokens, 100 priced tokens, complete daily coverage, and no pending
or error files. Its second pass read zero bytes and kept the same totals.

The public usage type signatures and token meanings still match. Claude Fable
5.1 and Mythos 5.1 prices already match the current table. Their September 1
release section now contains more API guidance; its price and cache rules are
unchanged. The review refreshes that section and window evidence.
The full public-source audit checks both providers' parser and pricing areas
and passes with 15 sources, so `reviewedAt` advances to 2026-09-05.

The reviewed set does not gate parsing. A version outside it is read with the
same structural checks, contributes its Observed Tokens, and leaves its Ranking
Day partial and unpriced until a fixture proves the shape. Claude Code ships
faster than this parser is reviewed, so a version gate would report zero tokens
for work that happened. Only an unreviewed version that also carries an
unreviewed usage shape withholds its counters.

The public package review on 2026-08-26 resolved each moving channel to an
exact package record and checked its npm `dist.integrity` value:

| Channel     | Package                          | Version   | Integrity                                                                                         |
| ----------- | -------------------------------- | --------- | ------------------------------------------------------------------------------------------------- |
| Stable      | `@anthropic-ai/claude-code`      | `2.1.231` | `sha512-1VG6CYH/x3M58L0wNYV2yLI3IPTCic+SXFrIw9IV2OrXA4EsMFAdXArWG87GpLZMkCCLculmrtDuJlwKLsysxg==` |
| Stable pair | `@anthropic-ai/claude-agent-sdk` | `0.3.231` | `sha512-tazYrn34/p9tNpzt2v5lkjQMO4ypnm52tQFFd2rUIFsmDfeaRZZp7laO6WeujFCenxAoDotFPlzr+y6uxwb0Ew==` |
| Latest      | `@anthropic-ai/claude-code`      | `2.1.246` | `sha512-E2PEKkal9D05dWnsc2fcPclJpEbJbnIE3D1vp33aPrsFbmdbqyNzEcc9/SeFIj53hvP/M5BuHygOFbeoBWEAOg==` |
| Latest pair | `@anthropic-ai/claude-agent-sdk` | `0.3.246` | `sha512-FtR0HoHHNqeqJWjZN8qLUAzZVFUI9ztXYNPPwv98Ecmv9qq2QTauI8IzkY26CC0mleWAqb9RQEW2C0OtiUliug==` |

The partial transcript review on 2026-09-02 also resolved the current latest
pair and checked its npm `dist.integrity` value:

| Channel     | Package                          | Version   | Integrity                                                                                         |
| ----------- | -------------------------------- | --------- | ------------------------------------------------------------------------------------------------- |
| Latest      | `@anthropic-ai/claude-code`      | `2.1.258` | `sha512-Zis1AYrHuCcK4V1tXJUkzJdklFsTvvqIcj7gk4K8lyEeJOW99ZoQH/E+WxugMqDEO7xncYZ41gydxlkSTmj/2Q==` |
| Latest pair | `@anthropic-ai/claude-agent-sdk` | `0.3.258` | `sha512-RxJ5fSPCGCxX5qO/b4IPXhldvtLHeYBAzTUJ4eOzO+gTrepZQSDmwSlQD6nnoEquKGJzOMHCjhdEtBfDjbDWUg==` |

This partial review updates only the Claude latest-package checkpoints. It
does not review the stable channel or any Codex area, so the shared
`reviewedAt` date does not change.

These public package records do not prove a private transcript shape. They do
not add a Claude Code version to the reviewed set.

The scanner ignores a synthetic API-error record in either of two reviewed
shapes: the earlier shape that carries no HTTP status, error details, or request
identifier, and the later shape that carries all three. The record must match a
reviewed shape exactly. This check
includes the wrapper, message, content, zero-token counters, zero paid-tool
counters, and null extended usage fields. The later reviewed shape also requires
an HTTP error status, non-empty error details, and a non-empty request
identifier. A different API-error shape fails closed. Transcript identity does
not gate known top-level counters: an unreviewed version with a reviewed usage
shape stays partial, while a record with both an unreviewed version and an
unreviewed usage shape withholds its counters.

The `2026-09-02-v1` review added Claude Fable 5.1 and Claude Mythos 5.1.
Anthropic
[launched both on 2026-09-01](https://platform.claude.com/docs/en/release-notes/overview).
That release note supplies the inclusive start date and states the reduced cache
read price. The periods stay open because no first-party source publishes a next
effective change. Claude Mythos 5.1 has limited availability through Project
Glasswing. Its public list price is the same and it is priced from the same
public table. Neither model appears in the Fast mode table, so both carry no
fast period. Both support `inference_geo`, so the 1.1 times US inference
modifier applies. These rates apply to one million tokens:

| Usage                |   USD |
| -------------------- | ----: |
| Input                | 10.00 |
| 5-minute cache write | 12.50 |
| 1-hour cache write   | 20.00 |
| Cache read           |  0.25 |
| Output               | 50.00 |

This review checked Claude pricing only. It did not review the reviewed Claude
Code version set, the Claude parser fixtures, or any Codex area, so the
`reviewedAt` checkpoint stays at its previous date.

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

The parser keeps non-null `fallback_credit` partial and unpriced. It accepts
one `iterations` entry only when its counters match the top-level counters.
It accepts `output_tokens_details.thinking_tokens` only when that value does
not exceed the inclusive output total. Other nested shapes keep known
top-level counters partial and unpriced. Review their billing meaning before
you extend these checks.

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

8. Add the exact Claude Code version to the reviewed set only after every
   fixture passes. Adding it is what lets that version's days claim complete
   coverage and a priced estimate; its tokens already count without it. Update the reviewed package versions, signatures, semantic markers,
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
