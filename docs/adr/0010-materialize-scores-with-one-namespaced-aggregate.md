# ADR 0010: Materialize Scores With One Namespaced Aggregate

## Status

Accepted

## Context

Computing rolling global rankings by scanning daily observations would become expensive and would make rank queries unbounded.

## Decision

Convex materializes 1-day, 7-day, and 30-day scores for Codex, Claude, and Combined scopes. The `publicUsages` table stores these public usage projections. A single `@convex-dev/aggregate` component installation partitions global boards by a versioned Board Key. Public Usage and Aggregate changes occur in the same mutation.

My Tokenmaxxers uses indexed, bounded reads followed by in-memory filtering and sorting. It does not use Aggregate.

## Consequences

Global rank reads remain logarithmic and rolling expiry requires a daily recomputation cron. Changes to score semantics require a new Board Key version and a migration rather than silently changing an existing ranking.

## Recompute work

One recomputation reads the Profile's existing Public Usage rows once for all
nine scope/window results. Cost, display values, and the computation time still
update in the same mutation. An unchanged ordering key and Board Key do not
require an Aggregate replacement. Check that exact Aggregate entry first so a
missing entry is repaired. Changed keys and namespaces retain the replacement
path. These rules do not change Token Score or rank semantics.
