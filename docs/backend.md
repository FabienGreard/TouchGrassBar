# Convex Backend

## Responsibility

Convex receives authenticated daily usage snapshots from the Rust native core and owns every public score. It never receives prompts, conversations, credentials, cookies, raw logs, or local paths. React has no Convex client or authentication material.

The deployed flow is:

`usageBuckets → userDailyUsage → publicUsages → doomerboard`

`deviceProviderSettings → filtered publicUsages recompute`

`usageBuckets` is the synchronization ledger. It owns Active Mac authority,
revisions, evidence, and correction provenance. `userDailyUsage` is the
canonical private history for one Tokenmaxxer, provider, and UTC day.
`publicUsages` materializes each public scope and window. The
`doomerboard` wraps `@convex-dev/aggregate` only for ordered
pagination. It does not calculate daily usage or rolling scores.

`packages/contracts` is not the sync contract. It is reserved for the sanitized Rust-to-React Tauri IPC boundary. Convex generates its own TypeScript API and data-model types in `convex/_generated`.

Rust is the only desktop Convex client. It exchanges its Keychain-held Better Auth session for a short-lived memory-only Convex JWT, then uses the official Convex Rust client through typed native operations. React receives sanitized results and has no Convex client, JWT, session material, or generic backend forwarding command.

## Snapshot invariant

One Usage Bucket represents one Active Mac generation, Coding Provider, and UTC Ranking Day. Rust sends a cumulative Daily Usage Snapshot with a monotonically increasing revision. The server treats an equal, exact payload as idempotent. A lower revision is stale. An equal revision with different data is also stale if its token total is not lower. The client rebases that snapshot to the server revision plus one. A snapshot is a conflict if its observation time moves backward or an equal revision has a lower unproved total. The client keeps that uncommitted request, records a terminal conflict for the exact revision, and stops retrying it. A new local observation can create a later revision. An older observation cannot overwrite a newer one. A higher revision may increase the total; a decrease is accepted only with an explicit provider-replacement or parser-correction reason. The request pairs that reason with the original correction revision. A later cumulative retry can identify the same correction without a second audit or authority for a new decrease. Disappearance of a local record is never valid downward evidence.

Each request contains at most 62 snapshots and commits atomically. A committed or idempotent acknowledgement names the submitted revision. A conflict result also names the submitted revision, but it does not acknowledge a commit. A stale acknowledgement names the same or a newer server revision. A timeout retry or concurrent duplicate is a no-op; one invalid snapshot rolls back the whole request. An accepted snapshot updates its Usage Bucket. The server then rebuilds the User Daily Usage value from every accepted Active Mac generation segment. It recomputes the derived score state in the same mutation. “Corrected” is audit provenance rather than a lasting public state. The client cannot submit a Tokenmaxxer ID, combined total, Token Score, rank, or public projection.

The native synchronization module also keeps one latest-only provider-setting
outbox row for the Active Mac generation. Its authenticated mutation stores a
monotonic enabled-provider revision before later usage delivery. Score
recomputation excludes accepted daily rows for disabled providers. The daily
facts stay retained and private to this projection path. A later re-enable
restores their valid 1-day, 7-day, and 30-day contribution without a new scan.
A stale provider-setting acknowledgement advances the local revision floor;
therefore, a late disable or re-enable request cannot restore an older setting.

On Active Mac transfer, the old generation's accepted contribution is frozen
and the new generation contributes only its post-transfer segment. Later writes
from the old generation fail. Ranking Days before the transfer day are not
rewritten. A known unsynchronized old segment makes the transfer day partial.

Native records the server-owned activation time. It captures a sanitized
baseline at installation only when the observation matches that time. Native
ignores earlier totals. The first later observation becomes a partial baseline.
Native subtracts compatible tokens and cost for the transfer day.

If authority installation occurs after a UTC rollover, Native does not relabel
current usage as transfer-day usage. It first sends one tagged transfer-day
carryover for each affected provider, with a maximum of two carryovers. A
carryover is either a zero-token partial record or an unacknowledged non-zero
partial segment that was observed after activation on that UTC day.

The backend accepts this historical exception only from the Active Mac
generation after the first generation. Its Ranking Day must equal the
server-owned device activation day and precede the current day. Its coverage
must be partial, and its observation time must be at or after activation and
remain within that day. A non-zero segment uses the normal token, cost,
correction, and revision rules. The zero-token marker must use the exact
activation time when Native creates it during delayed installation. A first
post-activation partial baseline can also leave a later zero-token carryover.
Both zero-token forms must use revision one and no cost or correction. The same
mutation records the carryover, rebuilds `userDailyUsage` for the transfer day
with unavailable API-Equivalent Cost, and recomputes the rolling scores.
A stale zero-token carryover is complete because the server has a newer
historical revision. Native removes it. A stale non-zero carryover rebases and
keeps its carryover tag.
Normal Usage Snapshots remain limited to the current UTC Ranking Day. The first
Active Mac can submit one atomic sparse Profile backfill for its server-owned
creation Ranking Day and the preceding 29 UTC days. This backfill has at most
60 provider-day rows. The request includes the server-owned creation Ranking
Day as a completion marker, including when no row is derivable. The marker and
all rows commit atomically. An exact retry is idempotent. After completion, a
later historical request normally updates only an existing generation-one
bucket at a higher revision. Missing days in the original backfill window stay
absent. A post-creation row that was first observed as current can retry after
its day closes only when `observedAt` is inside that exact UTC day. Later Active
Mac generations cannot use this authority. Other historical observations can
occur after their Ranking Day, but they cannot precede that day or exceed the
clock-skew limit.

## Usage-contract verification

Automated fixtures must cover Quota Lane freshness and reset transitions; provider/day source precedence; Codex and Claude token-category counting without overlap; complete, partial, stale, unavailable, and missing-not-zero behavior; revision idempotency and authorized downward corrections; effective-dated pricing and catalog-triggered cost recomputation; unknown-price omission; and the sanitized synchronization payload. Native-boundary gates additionally cover generated-contract drift and unknown-version rejection; sentinel secret, path, identifier, and raw-content exclusion from IPC and sync payloads; SQLite migration and atomic-outbox crash recovery; refresh timing with a fake clock; Active Mac revocation; and release CSP/capability restrictions. These fixtures define the acceptance contract but do not replace final release QA.

## Score materialization

Every accepted change writes `userDailyUsage` and recomputes the affected Tokenmaxxer's Codex, Claude, and Combined Token Scores for 1, 7, and 30 UTC days. Recompute reads at most 30 rows for each enabled provider through the Tokenmaxxer, provider, and Ranking Day index. It does not scan older history. The same mutation updates `publicUsages` and `doomerboard`. Each daily fact and score keeps the complete API-equivalent cost object: micros, quality, coverage, and pricing basis. Combined scores use the same conservative reduction as the native usage summary. They sum valid estimates, keep all pricing bases, use the weakest quality, and report token-weighted modeled coverage. An unpriced provider does not hide another provider's valid estimate.

The `@convex-dev/aggregate` component has one installation named `doomerboard`.
The local `doomerboard` symbol partitions score document IDs by keys
such as:

- `tokens-v1:codex:30d`
- `tokens-v1:claude:7d`
- `tokens-v1:combined:1d`

Doomerboards use the composite Aggregate key `[-TokenScore, TouchGrass ID]`.
Ascending Aggregate pagination therefore returns the highest score first and
uses TouchGrass ID as the deterministic tie break. The current Global query is
limited to the validated Codex, Claude, or Combined scope and the 1-, 7-, or
30-day window. It requires the live Profile and returns at most 100 public
rows. It accepts no client identity. The native caller sends its validated UTC
Ranking Day, scope, and window. The Ranking Day keeps the cached query stable
within a day and changes its argument at rollover.

`publicUsages` and `doomerboard` have one write path: every insert,
replacement, or deletion changes both within the same mutation. A legacy
compatibility read counts numeric Aggregate entries before it loads them and
fails closed above its fixed 640-row budget. This keeps deterministic
TouchGrass ID tie ordering without a new blocking database index. A bounded,
read-only invariant check proves a one-to-one match of document ID, Board Key,
and composite key across all stored namespaces. It returns counts only. The
paired repair is idempotent and changes only the observed index entry. The
`backfillDoomerboard` migration repairs legacy numeric keys and missing index
entries without recomputing usage or scores. Production dashboard edits to
either side are prohibited.

My Tokenmaxxers contains at most 100 saved Tokenmaxxers. The add mutation
rejects a new unique entry at that limit but keeps an existing entry
idempotent. Its query reads at most 101 indexed edges, fails closed if legacy
data exceeds the limit, performs at most 100 indexed score lookups, and sorts
only that bounded set in memory. An existing overflow can still be reduced by
the exact indexed remove mutation. It never scans the global score table. The
current-day response also reports the exact saved Tokenmaxxer count. The native
client accepts the board only when every saved entry has a current score, so an
empty saved list stays distinct from a partial or unavailable score set.

## Maintenance

A built-in daily cron starts at 00:05 UTC. It paginates until every Tokenmaxxer whose rolling score can change has been processed, so expired Ranking Days leave all windows. The drain is idempotent, retries safely, alerts if progress stalls, and has no correctness cutoff or fixed-record ceiling. Its launch-load fixture must remain within the approved backend performance budget.

The migrations component owns bounded repair work. The
`backfillDoomerboard` migration is forward-only, resumable, and
idempotent. It removes the legacy numeric key and inserts the deterministic
composite key for each `publicUsages` row.
The `backfillDeviceUsageCompletion` migration adds an explicit `null` pending
state to older Device documents before `usageBackfillCompletedAt` becomes a
required schema field. Missing and `null` use the same fail-closed behavior
during this bounded migration.

This feature requires the credential-based Active Mac and current usage schemas
directly. It has no compatibility migration for an earlier feature shape.
Reset a local development deployment if it contains that older shape. The
bounded Device completion-field migration applies only to current Active Mac
documents. This branch does not change a cloud deployment.

## Authentication boundary

Every protected operation calls one shared authorization guard. The guard validates the live Better Auth session, derives the Tokenmaxxer from the Better Auth user, and never accepts a client-supplied user identifier as authority. Synchronization additionally requires the server-owned Active Mac generation and installation credential. Transfer revokes every earlier session and generation immediately.

Better Auth generated credentials and the desktop session-to-JWT exchange are wired. Active Mac transfer and recovery remain separate implementation gates. Synchronization accepts only the current claimed installation credential and server-owned generation.

## Abuse policy

Rate limits and query caps live in one typed policy table whose boundary tests are generated from the same values. A policy change invalidates Backend Readiness Evidence.

- Synchronization accepts at most 62 snapshots per request. Its token bucket has capacity 180 and refills 60 snapshots per minute, keyed by Tokenmaxxer, Active Mac generation, and installation.
- Failed generated-credential or recovery attempts are limited to five per 15 minutes independently by IP and TouchGrass ID, with non-enumerating responses.
- Successful recovery or transfer is limited to three per hour per Tokenmaxxer.

An automated hostile-input suite rejects oversized payloads, invalid or future Ranking Days, unsafe numbers, unauthorized decreases, malformed installation credentials, and any raw identifier or path. Rejection never partially writes or reveals whether a Tokenmaxxer exists.

## Production readiness contract

Backend readiness is binary and automated. Local or development results do not qualify. Every mandatory check must pass; failed, skipped, or stale evidence blocks launch. Auth, privacy, authorization, data-integrity, migration, and canary failures cannot be waived.

The automated evidence set contains:

- generated-credential tests for one-time signup-proof expiry and replay rejection, Recovery Key hashing, session and JWT claims, immediate revocation, and secret exclusion from Convex data, logs, React, and artifacts;
- authorization tests for absent, expired, revoked, and mismatched sessions; wrong installations; stale Active Mac generations; and transfer/sync races;
- atomic synchronization tests for retries, duplicate and concurrent delivery, valid corrections, rollback, same-day transfer segmentation, and abandoned old-generation work;
- fake-clock UTC rollover tests across month, year, leap-day, and daylight-saving boundaries, plus a complete paginated-drain test;
- an independent reference oracle that applies randomized synchronization, correction, transfer, and rollover sequences and compares User Daily Usage, Public Usage projections, and Doomerboard index ranks;
- bounded-query and rate-limit boundary tests, hostile-input tests, and an interrupted migration rehearsal;
- a disposable authenticated canary against the exact production deployment before public visibility. It proves generated credentials, session/JWT exchange, Active Mac claim, synchronization and identical retry, public and private reads, transfer, old-Mac rejection, new-Mac synchronization, and complete internal cleanup without logging secrets; and
- a production health check for the exact deployment, presence-only required environment variables, installed schema and components, the canary's sanitized correlation window, zero unhandled backend errors, and the Public Usage/Doomerboard index invariant.

The resulting Backend Readiness Evidence is one machine-readable CI artifact containing the exact Git commit, dependency-lock hash, schema and Board Key versions, policy version, deployment identity, suite results, migration rehearsal, production-canary result, and production-health result. Relevant code, configuration, schema, dependency, or policy changes make it stale. Before real traffic exists, production evidence is explicitly labeled `canary-only`; post-launch monitoring is a separate operational gate.

## Validation

This document defines the target contract; it does not claim launch readiness. The issue 26 and issue 27 implementations have a typed current-day mutation, a bounded first-Profile backfill, live Better Auth authorization, Active Mac generation checks, correction provenance, and a native latest-revision outbox. Local tests do not prove a production deployment, authenticated canary, production Active Mac transfer or backfill, rollover completion, or release approval. Those items and regenerated Backend Readiness Evidence remain separate gates.
