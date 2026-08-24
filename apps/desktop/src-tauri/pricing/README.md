# Bundled API pricing

`openai-standard.json` and `anthropic-standard.json` are the offline price
sources for API-equivalent cost. The application embeds these files at compile
time. It does not download price updates.

To update prices, edit the manifest, change its `basis`, test the effective
date ranges, and release a new application version. On the first run of that
version, TouchGrassBar reprices retained private SQLite cost details. It does
not parse provider content again for a price-only change.

The index stores a semantic fingerprint of the validated manifest to detect an
update even if the basis was not changed by mistake. Each private priced detail
also stores the fingerprint of its applicable pricing rule. A manifest update
changes only aggregates whose applicable rule changed. The basis remains the
readable price-book version that the application can show in sanitized output.

The Rust parsers reject unknown fields and invalid manifests. Keep each format
suitable for a future signed remote manifest. Do not add remote updates as part
of the local usage work.

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

Use these primary sources for an OpenAI update:

- [OpenAI API pricing](https://developers.openai.com/api/docs/pricing)
- [OpenAI Fast mode](https://developers.openai.com/api/docs/guides/fast-mode)
- [OpenAI API changelog](https://developers.openai.com/api/docs/changelog)
- [OpenAI model catalog](https://developers.openai.com/api/docs/models)
- [GPT-5.6 Sol](https://developers.openai.com/api/docs/models/gpt-5.6-sol)

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

The Claude transcript allow-list has three verified pairs: Claude Code
`2.1.223` with `@anthropic-ai/claude-agent-sdk` `0.3.223`, Claude Code `2.1.224`
with `@anthropic-ai/claude-agent-sdk` `0.3.224`, and Claude Code `2.1.241` with
`@anthropic-ai/claude-agent-sdk` `0.3.241`. It ignores a Claude Code `2.1.241`
synthetic API-error record only when the reviewed wrapper, message, content,
zero-token counters, zero paid-tool counters, and null extended usage fields
match exactly. A different API-error shape fails closed. A different transcript
version is partial or unavailable until controlled fixtures prove that schema.
