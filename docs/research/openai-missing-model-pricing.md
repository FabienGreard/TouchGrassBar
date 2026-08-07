# OpenAI pricing for missing Codex model identifiers

**Status:** Current-source research; no pricing manifest change

**Date:** 2026-08-06
**Scope:** Model identifiers with no cost in the pre-change 60-day private
Codex usage snapshot. TouchGrassBar now retains 30 UTC days of local cost
detail. Only official OpenAI documentation and source code are used.

## Decision

Do not add a guessed price for `gpt-5.3-codex-spark` or
`codex-auto-review`.

- OpenAI lists GPT-5.3-Codex-Spark as a research preview and says its credit
  rates are not final. The model does not have a standard API price row.
  ([Codex rate card](https://help.openai.com/en/articles/20001106#h_73f3414088),
  [API pricing](https://developers.openai.com/api/docs/pricing))
- OpenAI defines `codex-auto-review` as the hidden model slug for the separate
  approval-review agent. OpenAI does not publish a price row or an underlying
  priced-model alias for this slug.
  ([official model catalog](https://github.com/openai/codex/blob/57f42a81131ccf5933e7ec5dc659c381eeb5d72b/codex-rs/models-manager/models.json#L749-L804),
  [Auto-review documentation](https://learn.chatgpt.com/docs/sandboxing/auto-review))
- `__unknown__` is a TouchGrassBar parser sentinel. It is not a model and must
  never receive a price or alias.
  ([local parser source](../../apps/desktop/src-tauri/src/providers/codex/usage.rs#L1004-L1037))

The safe row result for all three identifiers is **unpriced**. A period can
still show a modeled API-equivalent estimate when other priced local evidence
exists. A future manifest update can add a model only after OpenAI publishes a
direct price or a direct alias to a priced model.

## Evidence boundary

The local debug report identified these model names as unpriced. Private UTC
ranges and token-category totals stay in the local SQLite database and console.
They must not be copied into repository documents.

Observed tokens equal input plus output. Cached input is already inside input,
and reasoning output is already inside output. The Rust validator enforces this
arithmetic.

## GPT-5.3-Codex-Spark

OpenAI released Spark on 2026-02-12 as a research preview. The launch article
says it had a 128,000-token context window, a separate Codex rate limit, and
limited API access for design partners. It does not publish dollar prices,
cached-input prices, cache-write prices, or output prices.
([Spark launch](https://openai.com/index/introducing-gpt-5-3-codex-spark/))

The current Codex rate card still marks all three Spark token columns as
`research preview` and says that its rates are not final. It also states that
Codex does not charge for cache writes. That cache-write statement is a Codex
credit rule, not a published standard API dollar price.
([Codex rate card](https://help.openai.com/en/articles/20001106#h_73f3414088))

The public API catalog has a separate `gpt-5.3-codex` model. Its published
standard prices are $1.75 per million input tokens, $0.175 per million cached
input tokens, and $14 per million output tokens. The page lists only
`gpt-5.3-codex` as its alias. OpenAI describes Spark as a smaller model, not as
an alias of GPT-5.3-Codex. Therefore, the GPT-5.3-Codex prices cannot be applied
to Spark.
([GPT-5.3-Codex API model](https://developers.openai.com/api/docs/models/gpt-5.3-codex),
[Spark launch](https://openai.com/index/introducing-gpt-5-3-codex-spark/))

| Required field | Official result |
| --- | --- |
| Standard API input price | Not published |
| Standard API cached-input price | Not published |
| Standard API cache-write price | Not published |
| Standard API output price | Not published |
| Price effective date | Not published |
| Priced-model alias | Not published |
| Long-context price rule | Not published; the launch only states a 128,000-token context window |

## Codex auto-review

The official Codex model catalog defines `codex-auto-review` as **Codex Auto
Review**, describes it as the automatic approval review model, hides it from
normal selection, and marks it as supported in the API. The same entry states
a 272,000-token context window and a 1,000,000-token maximum. It does not name
an underlying model or contain pricing data.
([official model catalog](https://github.com/openai/codex/blob/57f42a81131ccf5933e7ec5dc659c381eeb5d72b/codex-rs/models-manager/models.json#L749-L804))

The product documentation says auto-review sends sandbox approval requests to
a separate reviewer agent. This is not the normal pull-request **Code review**
feature. The Codex rate card says that Code review uses GPT-5.3-Codex, but it
does not make the same statement for the approval-review agent. Therefore,
`codex-auto-review` cannot safely use GPT-5.3-Codex pricing.
([Auto-review documentation](https://learn.chatgpt.com/docs/sandboxing/auto-review),
[Codex rate card](https://help.openai.com/en/articles/20001106#h_73f3414088))

| Required field | Official result |
| --- | --- |
| Standard API input price | Not published |
| Standard API cached-input price | Not published |
| Standard API cache-write price | Not published |
| Standard API output price | Not published |
| Price effective date | Not published |
| Underlying priced-model alias | Not published |
| Long-context price rule | Not published; context capacity alone is not a pricing rule |

## Update rule

Recheck the official API model page, API pricing page, Codex rate card, and
official Codex model catalog when the debug report finds a new model. Add a
manifest row only when the source gives all prices that apply to the stored
token categories. If one required price or alias is missing, keep the cost
unavailable and keep the account token total visible.

## Primary sources

- [OpenAI API pricing](https://developers.openai.com/api/docs/pricing)
- [OpenAI API model catalog](https://developers.openai.com/api/docs/models/all)
- [GPT-5.3-Codex API model](https://developers.openai.com/api/docs/models/gpt-5.3-codex)
- [Codex rate card](https://help.openai.com/en/articles/20001106)
- [Introducing GPT-5.3-Codex-Spark](https://openai.com/index/introducing-gpt-5-3-codex-spark/)
- [Auto-review documentation](https://learn.chatgpt.com/docs/sandboxing/auto-review)
- [OpenAI Codex model catalog at `57f42a81`](https://github.com/openai/codex/blob/57f42a81131ccf5933e7ec5dc659c381eeb5d72b/codex-rs/models-manager/models.json#L749-L804)
