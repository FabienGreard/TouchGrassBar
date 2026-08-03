# Keep provider utility local-first

Provider detection, Quota Snapshots, local history, and API-Equivalent Cost remain available when Convex is unreachable. Rust caches sanitized state and pending Daily Usage Aggregates locally, while identity and Doomerboard features expose honest unavailable or stale states and resume synchronization after connectivity returns.
