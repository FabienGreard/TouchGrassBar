# Open Questions

These product and technical decisions remain unresolved. Resolve them before their answers become expensive to change.

## Doomerboard behavior

- How quickly must synchronized changes affect a Doomerboard under production Aggregate load?

## Distribution and operations

- What update endpoint, signed manifest format, and public key will automatic updates use?
- Which GitHub environments and approval rules protect Developer ID and notarization secrets?
- What are the release thresholds for cold startup, panel latency, idle CPU, memory, and app size?
