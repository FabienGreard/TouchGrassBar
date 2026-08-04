# ADR 0013: Use Snapshot, Refresh, and Revision Notices for Native State

## Status

Accepted

## Context

The trusted native core owns provider observation, freshness, pricing, persistence, synchronization, and privacy reduction. React needs current presentation state and a small number of user intents, but it must not reproduce native lifecycle rules or receive provider source material.

A command-per-feature interface would expose native implementation detail as the product grows. A generic intent bus would hide type safety behind a broad transport contract. Pushing partial state would require React to resolve ordering, missed events, and stale patches. Capability-bearing surface sessions could support more dynamic or untrusted clients, but add lease, capability, and event-lifecycle complexity that the trusted menu-bar surfaces do not currently need.

## Decision

The native core is a deep module with one snapshot-oriented interface for the menu-bar panel:

```rust
trait NativeCore {
    fn panel_state(&self) -> Result<SanitizedDesktopStateV1, CoreFault>;
    fn request_refresh(
        &self,
        source: RefreshSource,
    ) -> Result<RefreshReceipt, CoreFault>;
    fn revision_notices(&self) -> RevisionStream;
}
```

The Tauri adapter exposes the current cached Sanitized Desktop State, a manual refresh request, and a revision-notice event. It verifies the caller, maps transport failures to closed sanitized codes, and contains no provider, freshness, pricing, persistence, or synchronization policy.

`panel_state` performs no provider, network, Keychain, or disk I/O. `request_refresh` starts or joins the native single-flight refresh coordinator and acknowledges the request without claiming that refresh succeeded. The tray, scheduler, wake handling, and network recovery use the same coordinator through native call sites.

Each Sanitized Desktop State carries a breaking-change contract version, generation time, and monotonic state revision encoded without JavaScript precision loss. Rust emits a revision notice only after the corresponding state commit. React subscribes before its initial read, coalesces notices, refetches the complete snapshot, and replaces visible state only with a higher revision. Notices are invalidation hints: they may be missed, duplicated, delayed, or reordered without changing correctness.

The snapshot uses closed discriminated states rather than ambiguous nulls or zeroes. Missing Observed Usage is unavailable, not zero. A Quota Snapshot can be initialized only by a full provider report. Usage Evidence Basis, Usage Coverage, Usage Availability, freshness, and API-Equivalent Cost availability remain independent. Expected provider, parsing, network, and persistence failures are represented as sanitized unavailable, stale, retry, or synchronization state. Only caller authorization, lifecycle, serialization, contract, or internal-invariant failures reject the Tauri call, and raw error details never cross the seam.

Sanitized Rust DTOs are canonical. TypeScript types and strict validators are generated deterministically; React does not maintain a parallel hand-written interpretation. Contract versions advance for released contract changes, not merely because a placeholder scaffold existed. Unknown versions fail closed.

The implementation keeps provider source selection, full-versus-sparse quota handling, UTC aggregation, freshness and reset transitions, effective-dated pricing, corrections, SQLite transactions, retention, refresh coalescing, backoff, synchronization, and privacy reduction behind the interface.

Codex and Claude are separate true-external internal seams with production and fixture adapters because their observation semantics differ. The clock has system and deterministic-test adapters. SQLite is local-substitutable and remains an internal seam exercised through temporary or in-memory SQLite rather than a generic repository port. Convex and Keychain use production and test adapters when their integrations are introduced.

Large Doomerboards use a separate typed, bounded query interface. Settings, Profile, recovery, updates, and future focused surfaces receive their own narrow interfaces instead of inflating Sanitized Desktop State or introducing a generic dispatcher.

## Consequences

React has one complete, revisioned view of panel state and cannot accidentally become the source of truth for freshness, retry, pricing, correction, or provider-specific behavior. Tests can drive the native core interface with fixture providers, a deterministic clock, and real temporary SQLite while asserting only product-visible state and revision ordering.

The Sanitized Desktop State remains a deliberately bounded projection and must not become an application-wide object graph. Adding a new focused surface may require another small interface. If future clients become untrusted, independently connected, or highly interactive, capability-bearing surface sessions can be reconsidered with evidence from at least two real adapters.
