---
status: accepted
---

# Separate Provider Observation from Daily Usage Aggregation

The compiled Provider Registry owns Provider Presence and stable display identity. Each Coding Provider has one deep observation adapter. The adapter owns authentication, provider payloads, local evidence, pricing, and private SQLite records. It returns complete sanitized observations after each bounded step, can omit unsupported Quota or usage capabilities, and does not expose model details, token categories, file metadata, paths, or raw content.

Daily Usage Aggregate calculation is a pure shared module. It receives normalized per-Ranking-Day evidence and a supplied clock, applies provider-reported precedence without adding local tokens, calculates cost coverage and period totals, and produces per-provider and Combined projections. Provider adapters can differ, but Codex, Claude, and future providers use this same calculation after normalization.

The native coordinator owns triggers, single-flight refresh, retries, backoff, provider isolation, and progressive complete-snapshot publication. Provider-private transactions commit before a new Sanitized Desktop State revision. The public contract uses an ordered dynamic provider collection; provider-private facts and the complete Sanitized Desktop State remain the only stored layers.

Codex is the only production usage adapter when this decision is accepted. Fixture adapters prove the small shared interface, but the seam remains provisional until the separate Claude observation work supplies a second production usage adapter. TouchGrassBar must not add an empty Claude usage adapter to make the seam appear complete.
