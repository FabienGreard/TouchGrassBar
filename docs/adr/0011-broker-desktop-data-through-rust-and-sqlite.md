# Broker desktop data through Rust and SQLite

The bounded diagnostic file queue in
[ADR-0021](0021-send-bounded-failure-diagnostics.md) is separate from the
product database so it can record database-open failures. Rust owns this
queue and its network transport. It is not a second store of product state.

TouchGrassBar keeps production WebViews network-dark and brokers provider, Keychain, filesystem, and Convex access through Rust instead of using the conventional Convex React client. Canonical Rust DTOs generate the versioned sanitized IPC contract, while one Rust-owned SQLite database commits local read models, revisions, and a generation-scoped synchronization outbox transactionally. This adds native transport and binding-generation work, but preserves one auditable privacy boundary, keeps session material out of React, and prevents crashes, retries, or Active Mac transfer from separating an aggregate from its pending revision.
