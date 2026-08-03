# Convex Backend

## Responsibility

Convex receives authenticated daily usage snapshots from the Rust native core and owns every public score. It never receives prompts, conversations, credentials, cookies, raw logs, or local paths. React has no Convex client or authentication material.

The deployed flow is:

`usageBuckets → userDailyUsage → userScores → publicScores → @convex-dev/aggregate`

`packages/contracts` is not the sync contract. It is reserved for the sanitized Rust-to-React Tauri IPC boundary. Convex generates its own TypeScript API and data-model types in `convex/_generated`.

## Snapshot invariant

One Usage Bucket represents one Active Mac, Coding Provider, and UTC Ranking Day. Rust sends a cumulative total with a monotonically increasing revision. The server ignores an equal or lower revision, so retries are idempotent and an older observation cannot overwrite a newer one.

The current Active Mac's accepted snapshot replaces the corresponding User Daily Usage value. The client cannot submit a Tokenmaxxer ID, combined total, Token Score, rank, or public projection.

## Score materialization

Every accepted change recomputes the affected Tokenmaxxer's Codex, Claude, and Combined Token Scores for 1, 7, and 30 UTC days. The same mutation updates both `publicScores` and the global Aggregate entry.

The Aggregate component has one installation named `doomerboard`. It partitions scores by keys such as:

- `tokens-v1:codex:30d`
- `tokens-v1:claude:7d`
- `tokens-v1:combined:1d`

Global Doomerboards page the Aggregate in descending order. My Tokenmaxxers reads at most 500 unilateral edges and at most 2,000 materialized rows for one board, then filters and sorts in memory. It does not use Aggregate.

## Maintenance

A built-in daily cron runs at 00:05 UTC. It selects at most 200 Tokenmaxxers active in the previous 45 days and schedules an isolated score recomputation for each one so expired Ranking Days leave rolling windows.

The migrations component owns repair/backfill work. The first migration can restore missing Aggregate entries from `publicScores` without duplicating existing entries.

## Authentication boundary

Every identity or synchronization mutation requires `ctx.auth`. The subject resolves the Tokenmaxxer; the client never chooses that relationship. The first successful sync binds an installation as the Active Mac, and another installation is rejected until an explicit recovery transfer revokes or replaces that authority.

Better Auth is pinned but its generated Recovery Key adapter and device-transfer flow remain an implementation gate. Until that is wired, the authenticated mutations are deliberately inaccessible to the desktop scaffold.

## Validation

The backend has been generated and pushed successfully to an anonymous local Convex deployment. Production deployment, Better Auth environment configuration, and a realistic authenticated sync invocation remain release gates.
