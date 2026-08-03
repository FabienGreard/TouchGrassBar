# ADR 0009: Use Revisioned Cumulative Usage Snapshots

## Status

Accepted

## Context

Background refresh, retries, corrections, and temporary network failures can deliver the same local observation more than once or out of order. Accepting token increments would double-count retries and make recovery ambiguous.

## Decision

Rust sends one cumulative snapshot per Active Mac, Coding Provider, and UTC Ranking Day. Each snapshot carries a monotonically increasing revision. Convex ignores equal or lower revisions and replaces server-derived daily state only after accepting a newer revision.

## Consequences

Synchronization is idempotent and stale writes cannot roll a bucket backward. Rust must persist revisions with its local cache. Identity recovery during the current UTC day still needs an explicit authority-transfer rule.
