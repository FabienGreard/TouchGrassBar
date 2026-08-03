# ADR 0010: Materialize Scores With One Namespaced Aggregate

## Status

Accepted

## Context

Computing rolling global rankings by scanning daily observations would become expensive and would make rank queries unbounded.

## Decision

Convex materializes 1-day, 7-day, and 30-day scores for Codex, Claude, and Combined scopes. A single `@convex-dev/aggregate` component installation partitions global boards by a versioned Board Key. Public Score and Aggregate changes occur in the same mutation.

My Tokenmaxxers uses indexed, bounded reads followed by in-memory filtering and sorting. It does not use Aggregate.

## Consequences

Global rank reads remain logarithmic and rolling expiry requires a daily recomputation cron. Changes to score semantics require a new Board Key version and a migration rather than silently changing an existing ranking.
