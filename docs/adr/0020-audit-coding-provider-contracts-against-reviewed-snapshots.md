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

The report status is `pass`, `review-required`, or `unavailable`. A version, schema, token category, model, rate, date, or modifier change is `review-required`. A missing, timed-out, oversized, redirected, or malformed source is `unavailable`, not a pass. Each non-pass report includes an explicit bounded reason code. Unknown usage fields and unknown models remain visible as review items. An unknown price leaves the related Observed Tokens unpriced and cannot create an API-Equivalent Cost from another model or Coding Provider.

Remote source data cannot change a reviewed snapshot, parser allow-list, parser fixture, production parser, price manifest, dependency, or runtime setting. A maintainer must review every change. A new local transcript version requires a controlled synthetic fixture because a public SDK contract does not prove a private on-disk transcript shape. A price-period change requires a dated first-party source for its effective start. A statement that a price is valid at least through a date is not an exclusive effective end. The maintainer adds reviewed fixtures or manifest periods, runs the applicable tests, and ships the change in a normal release.

The single `reviewedAt` date may advance only after one review checks every Codex and Claude parser and pricing area. A partial review must keep the existing date. Provider or price changes can still update their own reviewed snapshot values without resetting the full-review clock.

The audit keeps remote bodies only for the bounded check and does not publish them. Its report may contain status, reason codes, first-party source URLs, versions, model IDs, semantic hashes, changed field names and signatures, rates, modifiers, and effective dates. It must not contain prompts, responses, credentials, authorization headers, account data, local paths, session IDs, raw JSONL records, or package source text.

The scheduled workflow publishes only this bounded report. For a non-pass result, it opens or updates one generated GitHub issue identified by a fixed hidden marker. It does not create an issue for each run or each source. A later pass against the reviewed snapshot can close that generated issue. The workflow never opens an automatic parser or pricing change.
