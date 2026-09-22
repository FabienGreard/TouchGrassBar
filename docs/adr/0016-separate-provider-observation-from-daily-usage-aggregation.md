---
status: accepted
---

# Separate Provider Observation from Daily Usage Aggregation

The compiled Provider Registry owns Provider Presence and stable display identity. Each Coding Provider has one deep observation adapter. The adapter owns authentication, provider payloads, local evidence, pricing, and private SQLite records. It returns complete sanitized observations after each bounded step, can omit unsupported Quota or usage capabilities, and does not expose model details, token categories, file metadata, paths, or raw content.

Daily Usage Aggregate calculation is a pure shared module. It receives normalized per-Ranking-Day evidence and a supplied clock. It never adds provider and local tokens.

Each provider and Ranking Day selects one source. A returned account bucket
has no completeness or finality marker. Its fetch time cannot prove that a
lower count corrects validated local response records. Select the account
bucket when it is at least as large as the available local count. Otherwise,
select the local count with its own evidence basis and coverage. Never add the
two counts. This rule also applies to returned zero and sparse responses.
If neither source has valid evidence, the day is unavailable.

The selection time is the later of the selected local record time and the
account bucket observation time. It records reconciliation of the sources;
it does not claim a new token event. Daily history, period totals, and upload
use the same selection. The native queue and backend accept a larger Codex
local count at the same or a later observation time. Revision and Active Mac
checks still apply. A completed Codex replay can retain its parser-correction
marker when it increases a saved account total. This does not permit a
decrease from account evidence or reuse of an old correction marker for a
second decrease. An explicit parser correction can reduce a local total
only after the relevant local scan completes. An ordinary partial scan or
missing local record cannot subtract previously synchronized usage.

Price completeness does not control token selection. Local evidence supplies
reconciled, modeled, or local-only cost for the selected source.

The module calculates cost coverage and period totals. It produces per-provider and Combined projections after source selection.

The native coordinator owns triggers, single-flight refresh, retries, backoff, provider isolation, and progressive complete-snapshot publication. Provider-private transactions commit before a new Sanitized Desktop State revision. The public contract uses an ordered dynamic provider collection.

The native store has three separate layers. Provider-private indexes keep private provider evidence. The Sanitized Desktop State is the public read model. The usage synchronization ledger keeps device authority, generations, baselines, revisions, correction lineage, and the latest bounded outbox. The ledger does not calculate provider observations or rolling windows.

The provider daily cache records an observation time for each returned bucket and records the last successful account refresh separately. A sparse refresh upserts the returned buckets and preserves other retained provider buckets. It does not refresh, delete, or set an omitted bucket to zero. An explicit zero bucket is provider evidence. A returned bucket follows the same comparison with local evidence; the cache does not claim that the bucket is final.

Codex scans the retained trace window once. Later refreshes read only appended rows from a monotonic SQLite cursor. The private memo checks database identity, deleted source rows, UTC coverage, and bounded evidence. A trace failure preserves the last committed pricing evidence.

Fast pricing requires a trusted response request or legacy provider submission. Completion evidence can refine the model, but it cannot prove Fast by itself.

Copied Codex history resolves through one trusted parent. The private index stores the parent snapshot dependency and rebuilds the child when that dependency changes. Ambiguous or unstable lineage stays excluded and makes coverage partial.

An incomplete local history schedules another bounded pass after 250 milliseconds. A failed pass waits 60 seconds. Provider observations continue to publish independently during catch-up.

Codex and Claude each have one production observation adapter. Both adapters use the same small shared interface. Each adapter keeps its provider-specific evidence and accounting rules behind that interface.

Codex response records supply individual response usage, including compaction.
The parser checks task ownership and token arithmetic, then stores the last
response identifier, usage, and cumulative response total with the file cursor.
Repeated responses and older cumulative records add no usage. A cumulative
gap keeps only the known individual response and makes coverage partial.
After response accounting starts, legacy cumulative notifications remain
lineage evidence but add no tokens. A resumed task therefore does not recount
its previous cumulative history. A transition from legacy records requires a
matching cumulative boundary; an ambiguous boundary stays partial.

Parser 25 replays all retained older checkpoints. Historical rows cannot use
the old parser promotion shortcut because their counts can also change.

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
