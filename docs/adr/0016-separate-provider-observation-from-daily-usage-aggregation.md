---
status: accepted
---

# Separate Provider Observation from Daily Usage Aggregation

The compiled Provider Registry owns Provider Presence and stable display identity. Each Coding Provider has one deep observation adapter. The adapter owns authentication, provider payloads, local evidence, pricing, and private SQLite records. It returns complete sanitized observations after each bounded step, can omit unsupported Quota or usage capabilities, and does not expose model details, token categories, file metadata, paths, or raw content.

Daily Usage Aggregate calculation is a pure shared module. It receives normalized per-Ranking-Day evidence and a supplied clock. It never adds provider and local tokens.

Provider-reported tokens are authoritative for each provider and Ranking Day when a provider bucket exists. Valid local evidence is the fallback only for a day that has no provider bucket. An omitted bucket is not an explicit zero. If neither source has valid evidence, the day is unavailable. The selector never adds provider and local tokens for the same day. A period can contain both evidence bases after the module folds the selected days. Local evidence also supplies reconciled or modeled cost. Price completeness does not control token authority.

The module calculates cost coverage and period totals. It produces per-provider and Combined projections after source selection.

The native coordinator owns triggers, single-flight refresh, retries, backoff, provider isolation, and progressive complete-snapshot publication. Provider-private transactions commit before a new Sanitized Desktop State revision. The public contract uses an ordered dynamic provider collection.

The native store has three separate layers. Provider-private indexes keep private provider evidence. The Sanitized Desktop State is the public read model. The usage synchronization ledger keeps device authority, generations, baselines, revisions, correction lineage, and the latest bounded outbox. The ledger does not calculate provider observations or rolling windows.

The provider daily cache records an observation time for each returned bucket and records the last successful account refresh separately. A sparse refresh upserts the returned buckets and preserves other retained provider buckets. It does not refresh, delete, or set an omitted bucket to zero. An explicit zero bucket is provider evidence. Provider evidence that arrives later replaces the local fallback for that Ranking Day, even when the provider total is lower.

Codex scans the retained trace window once. Later refreshes read only appended rows from a monotonic SQLite cursor. The private memo checks database identity, deleted source rows, UTC coverage, and bounded evidence. A trace failure preserves the last committed pricing evidence.

Fast pricing requires a trusted response request or legacy provider submission. Completion evidence can refine the model, but it cannot prove Fast by itself.

Copied Codex history resolves through one trusted parent. The private index stores the parent snapshot dependency and rebuilds the child when that dependency changes. Ambiguous or unstable lineage stays excluded and makes coverage partial.

An incomplete local history schedules another bounded pass after 250 milliseconds. A failed pass waits 60 seconds. Provider observations continue to publish independently during catch-up.

Codex and Claude each have one production observation adapter. Both adapters use the same small shared interface. Each adapter keeps its provider-specific evidence and accounting rules behind that interface.
