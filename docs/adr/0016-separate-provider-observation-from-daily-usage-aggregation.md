---
status: accepted
---

# Separate Provider Observation from Daily Usage Aggregation

The compiled Provider Registry owns Provider Presence and stable display identity. Each Coding Provider has one deep observation adapter. The adapter owns authentication, provider payloads, local evidence, pricing, and private SQLite records. It returns complete sanitized observations after each bounded step, can omit unsupported Quota or usage capabilities, and does not expose model details, token categories, file metadata, paths, or raw content.

Daily Usage Aggregate calculation is a pure shared module. It receives normalized per-Ranking-Day evidence and a supplied clock. It never adds provider and local tokens.

Provider-reported tokens are authoritative for each provider and Ranking Day when the latest response returns that provider bucket. Valid local evidence is the fallback for a day with no provider bucket, or for an older cached bucket under the rule below. An omitted bucket is not an explicit zero. If neither source has valid evidence, the day is unavailable. The selector never adds provider and local tokens for the same day. A period can contain both evidence bases after the module folds the selected days. Local evidence also supplies reconciled or modeled cost. Price completeness does not control token authority.

An omitted day can retain an older account bucket in the private cache. If a
later successful account response omits that day, local evidence can replace
the cached bucket in the projection only when its count is greater and its
last usage event is after the bucket's observation time. A new scan time alone
does not meet this condition. The cache keeps the account evidence. The
selected local count keeps its local evidence basis and coverage. A returned
account bucket takes priority again, including a lower correction or zero.
This prevents an omitted account day from holding the display at an old count
while new local usage accumulates. Daily history and period totals use the same
selection rule.

The module calculates cost coverage and period totals. It produces per-provider and Combined projections after source selection.

The native coordinator owns triggers, single-flight refresh, retries, backoff, provider isolation, and progressive complete-snapshot publication. Provider-private transactions commit before a new Sanitized Desktop State revision. The public contract uses an ordered dynamic provider collection.

The native store has three separate layers. Provider-private indexes keep private provider evidence. The Sanitized Desktop State is the public read model. The usage synchronization ledger keeps device authority, generations, baselines, revisions, correction lineage, and the latest bounded outbox. The ledger does not calculate provider observations or rolling windows.

The provider daily cache records an observation time for each returned bucket and records the last successful account refresh separately. A sparse refresh upserts the returned buckets and preserves other retained provider buckets. It does not refresh, delete, or set an omitted bucket to zero. An explicit zero bucket is provider evidence. Provider evidence that arrives later replaces the local fallback for that Ranking Day, even when the provider total is lower.

Codex scans the retained trace window once. Later refreshes read only appended rows from a monotonic SQLite cursor. The private memo checks database identity, deleted source rows, UTC coverage, and bounded evidence. A trace failure preserves the last committed pricing evidence.

Fast pricing requires a trusted response request or legacy provider submission. Completion evidence can refine the model, but it cannot prove Fast by itself.

Copied Codex history resolves through one trusted parent. The private index stores the parent snapshot dependency and rebuilds the child when that dependency changes. Ambiguous or unstable lineage stays excluded and makes coverage partial.

An incomplete local history schedules another bounded pass after 250 milliseconds. A failed pass waits 60 seconds. Provider observations continue to publish independently during catch-up.

Codex and Claude each have one production observation adapter. Both adapters use the same small shared interface. Each adapter keeps its provider-specific evidence and accounting rules behind that interface.

Codex version review is diagnostic information. An unknown version does not
block valid Observed Usage. The parser selects legacy or paginated record
ordering from the record structure. It still checks counter arithmetic,
record order, history boundaries, and copied-history ownership. A parser
revision change retries files that an earlier version gate rejected.

## Claude record recovery

Claude source versions affect the completeness claim, not whether validated
outer token counters contribute. Unknown metadata and inconsistent repeated
counters leave known usage partial and unpriced. The parser does not add
repeated counters or guess unknown token categories. Invalid required fields,
invalid counter types, and overflow still exclude a record. Parser revision 12
replays older file checkpoints, including files with previously excluded usage.
