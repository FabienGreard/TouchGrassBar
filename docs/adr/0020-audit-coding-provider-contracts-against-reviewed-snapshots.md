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
from an unreviewed version with a reviewed shape keeps its known counters and
leaves the day partial. A reviewed-version record with an unreviewed shape also
keeps the top-level counters whose meaning that version established, ignores
the unknown fields, and leaves the day partial. The parser never adds an
unknown field to Observed Tokens. Only a record whose version and shape are both
unreviewed withholds its counters, because nothing has then checked what those
counters mean. Reporting zero tokens for work that happened is a worse failure
than reporting known counters as partial: an omitted bucket is not proof of
zero usage.

The same rule limits `deny_unknown_fields`. Keep it where an unknown field can
change the meaning of the counters the parser reads or where the parser cannot
separate a total from a repeated subset. The Claude top-level usage object is
an explicit exception: it reads only named reviewed counters, never adds an
unknown field, records whether the shape is reviewed, and applies the
version-and-shape rule above. Do not use `deny_unknown_fields` where an added
sibling cannot change the meaning of the fields already read. This includes
the Codex quota payloads: a Quota Lane reports a provider-defined percentage,
and blanking it over an unrelated new field serves nobody. Those payloads
record unknown keys in a bounded, value-free collector instead and keep
serving their lanes. A debug build also logs the drift once per payload kind as
a development aid. The audit, not the running installation, is what tells a
maintainer that a payload changed, because it reads the published provider
schemas; a release build reports no runtime drift.

The Claude `/usage` reading follows from the same distinction. Headings,
ordering, and decoration are presentation that changes without the quota
changing, so the reading identifies a candidate and its horizon from the
percentage-and-reset shape. Heading text is never a parse gate. When a plan
shows two weekly candidates with the same shape, the compacted all-model marker
is a bounded preference for the supported provider-wide window. A missing or
renamed marker falls back to shape. A window the parser cannot read leaves its
own lane out instead of discarding the window it could read.

This amendment does not relax review. It moves the consequence of an unreviewed
provider release from an outage to a visible partial result, and leaves the
audit as the mechanism that tells a maintainer to review.

The report status is `pass`, `review-required`, or `unavailable`. A version, schema, token category, model, rate, date, or modifier change is `review-required`. A missing, timed-out, oversized, redirected, or malformed source is `unavailable`, not a pass. Each non-pass report includes an explicit bounded reason code. Unknown usage fields and unknown models remain visible as review items. An unknown price leaves the related Observed Tokens unpriced and cannot create an API-Equivalent Cost from another model or Coding Provider.

Remote source data cannot change a reviewed snapshot, parser allow-list, parser fixture, production parser, price manifest, dependency, or runtime setting. A maintainer must review every change. A new local transcript version requires a controlled synthetic fixture because a public SDK contract does not prove a private on-disk transcript shape. A price-period change requires a dated first-party source for its effective start. A statement that a price is valid at least through a date is not an exclusive effective end. The maintainer adds reviewed fixtures or manifest periods, runs the applicable tests, and ships the change in a normal release.

The single `reviewedAt` date may advance only after one review checks every Codex and Claude parser and pricing area. A partial review must keep the existing date. Provider or price changes can still update their own reviewed snapshot values without resetting the full-review clock.

The audit keeps remote bodies only for the bounded check and does not publish them. Its report may contain status, reason codes, first-party source URLs, versions, model IDs, semantic hashes, changed field names and signatures, rates, modifiers, and effective dates. It must not contain prompts, responses, credentials, authorization headers, account data, local paths, session IDs, raw JSONL records, or package source text.

The scheduled workflow publishes only this bounded report. For a non-pass result, it opens or updates one generated GitHub issue identified by a fixed hidden marker. It does not create an issue for each run or each source. A later pass against the reviewed snapshot can close that generated issue. The workflow never opens an automatic parser or pricing change.
