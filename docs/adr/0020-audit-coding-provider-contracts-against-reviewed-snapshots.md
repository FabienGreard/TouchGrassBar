---
status: accepted
---

# ADR 0020: Audit Coding Provider Contracts Against Reviewed Snapshots

TouchGrassBar keeps a development-time contract audit for Codex and Claude. The audit runs once each week and by manual command. It reads only allow-listed, public provider and official registry HTTPS sources. It does not use provider credentials, run an installed Coding Provider, send a model request, or read provider account data or local history. The audit is not part of runtime provider observation.

The audit is one deep module with a bounded report interface. Internal source
adapters read mutable release channels. They then fetch the generated Codex
schemas for the returned release tag. They compare schema hashes, Codex event
kinds, token mappings, Claude field signatures, and token-meaning markers.
They also compare parser version ranges, model IDs, current rates, and
modifiers.

The OpenAI adapter compares Standard and Fast token rates. It also checks the
reviewed Fast-mode and regional-processing statements. This can detect a
change between an inclusive total and an additive detail. Semantic manifest
hashes detect an unreviewed local rate or effective-date change. Bounded rule
windows detect added pricing surcharges and modifiers.

The audit binds each reviewed price change to a dated first-party section.
Every manifest date boundary needs evidence or an explicit legacy exemption.
Section and bounded-window hashes detect later corrections that retain old
statements. The pricing runbook records this evidence. The module does not use
a complete HTML page hash as a contract. Layout and navigation can change
without a semantic change.

## Amendment: fail closed on shape, not on provider identity

A reviewed snapshot pins what a provider payload looked like when a maintainer
checked it. Two kinds of check follow from that snapshot, and they behave very
differently over time.

A _shape_ check asks whether the payload still has the structure this parser
read. It fails only when something real changed, so a maintainer sees a true
signal. A _identity_ check asks whether the payload came from a version, build,
or label the maintainer has seen. It fails on every provider release, whether or
not anything changed. Claude Code and Codex both ship faster than this project
reviews them, so an identity check makes an ordinary provider release an
outage.

Identity therefore does not gate observation. The reviewed Claude Code version
set decides whether a Ranking Day can claim complete coverage and a priced
estimate. It does not decide whether that day's Observed Tokens exist. A record
from an unreviewed version is read with the same structural checks; it keeps its
tokens and leaves the day partial. Only a record that is unreviewed _and_
carries an unreviewed shape withholds its counters, because nothing has then
checked what those counters mean. Reporting zero tokens for work that happened
is a worse failure than reporting them as partial: an omitted bucket is not
proof of zero usage.

The same rule sorts `deny_unknown_fields`. Keep it wherever an unknown field
could be a subset that double-counts, which is every struct feeding Observed
Tokens. Do not use it where an added sibling cannot change the meaning of the
fields already read, which includes the Codex quota payloads: a Quota Lane
reports a provider-defined percentage, and blanking it over an unrelated new
field serves nobody. Those payloads record unknown keys in a bounded, value-free
collector instead, keep serving their lanes, and report the drift as a review
signal.

The Claude `/usage` reading follows from the same distinction. Headings,
ordering, and decoration are presentation that changes without the quota
changing, so the reading matches each window by its own shape rather than by
label text, and a window it cannot read leaves its own lane out instead of
discarding the window it could read.

This amendment does not relax review. It moves the consequence of an unreviewed
provider release from an outage to a visible partial result, and leaves the
audit as the mechanism that tells a maintainer to review.

The report status is `pass`, `review-required`, or `unavailable`. A version, schema, token category, model, rate, date, or modifier change is `review-required`. A missing, timed-out, oversized, redirected, or malformed source is `unavailable`, not a pass. Each non-pass report includes an explicit bounded reason code. Unknown usage fields and unknown models remain visible as review items. An unknown price leaves the related Observed Tokens unpriced and cannot create an API-Equivalent Cost from another model or Coding Provider.

Remote source data cannot change a reviewed snapshot, parser allow-list, parser fixture, production parser, price manifest, dependency, or runtime setting. A maintainer must review every change. A new local transcript version requires a controlled synthetic fixture because a public SDK contract does not prove a private on-disk transcript shape. A price-period change requires a dated first-party source for its effective start. A statement that a price is valid at least through a date is not an exclusive effective end. The maintainer adds reviewed fixtures or manifest periods, runs the applicable tests, and ships the change in a normal release.

The single `reviewedAt` date may advance only after one review checks every Codex and Claude parser and pricing area. A partial review must keep the existing date. Provider or price changes can still update their own reviewed snapshot values without resetting the full-review clock.

The audit keeps remote bodies only for the bounded check and does not publish them. Its report may contain status, reason codes, first-party source URLs, versions, model IDs, semantic hashes, changed field names and signatures, rates, modifiers, and effective dates. It must not contain prompts, responses, credentials, authorization headers, account data, local paths, session IDs, raw JSONL records, or package source text.

The scheduled workflow publishes only this bounded report. For a non-pass result, it opens or updates one generated GitHub issue identified by a fixed hidden marker. It does not create an issue for each run or each source. A later pass against the reviewed snapshot can close that generated issue. The workflow never opens an automatic parser or pricing change.
