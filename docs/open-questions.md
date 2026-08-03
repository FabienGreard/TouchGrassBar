# Open Questions

These product and technical decisions remain unresolved. Resolve them before their answers become expensive to change.

## Product semantics

- Can the local quota view be used without creating a public Tokenmaxxer identity, or is identity required before any app functionality is available?

## Provider truth

- Which Codex and Claude sources can provide quota allowance, remaining usage, reset time, and observed usage without reading or uploading prohibited content?
- How does the interface distinguish unavailable, stale, estimated, and provider-authoritative values?
- Which model-specific input, output, cache, and reasoning rates contribute to API-Equivalent Cost?
- How are historical corrections handled after a daily aggregate has synchronized?

## Identity and synchronization

- What Better Auth credential adapter maps the generated Recovery Key to a Convex `ctx.auth` subject without exposing session material to React?
- During an Active Mac transfer in the middle of a UTC day, does the new Mac replace the previous device's cumulative day snapshot immediately, or does the backend retain an explicit transfer boundary?
- How is the previous Mac's cached session invalidated quickly while still rejecting later offline writes by authority on the server?

## Doomerboard behavior

- How quickly must synchronized changes affect a Doomerboard under production Aggregate load?

## Distribution and operations

- What update endpoint, signed manifest format, and public key will automatic updates use?
- Which GitHub environments and approval rules protect Developer ID and notarization secrets?
- What are the release thresholds for cold startup, panel latency, idle CPU, memory, and app size?
