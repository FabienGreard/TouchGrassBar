---
status: accepted
---

# Coordinate Forward SQLite Compatibility

One Rust database coordinator owns SQLite readiness. It first inspects the global format, every registered module version, and the known object shapes without a write. It rejects an unknown or newer format before it creates a backup or starts a migration.

For a known older format, the coordinator creates and verifies one durable full-database backup. A durable marker identifies a migration retry. Before a later migration starts, the coordinator atomically replaces the completed migration's backup with the latest source database. It then runs registered module migrations in a fixed order and checks the complete structural and domain invariants. A failure keeps the backup, stops startup, and does not delete, reset, or partly accept the database. A successful open removes the marker and returns an opaque Ready token. Native persistence, provider work, synchronization, and update checks require this token.

Every persisted module has an explicit current schema version. A current release increases a module version when its format changes. This lets an older release detect a database from a newer release and fail closed without a write. Legacy column inspection can identify only listed historical formats. It cannot infer an unknown format.

## Implementation map

The coordinator has one interface: `prepare(path) -> PreparedDatabase`.
Callers open the database only after they receive the token.

```text
apps/desktop/src-tauri/src/database/
├── mod.rs                       external interface and readiness flow
├── catalog.rs                   accepted versions and schema shapes
├── inspection.rs                read-only source preflight
├── migration.rs                 migration order, backup, and recovery
├── invariants.rs                final structural and domain checks
└── tests/
    ├── mod.rs                   coordinator behavior tests
    └── release_compatibility.rs historical release fixture tests
```

Each data-owning module implements its schema migrations. `migration.rs` owns
their order and the shared backup and recovery protocol.

Dependencies point toward `catalog`: `inspection -> catalog`, `invariants ->
inspection/catalog`, and `migration -> inspection/invariants/catalog`.
Cross-file implementation items stay `pub(super)`; the interface stays in
`mod.rs`.

## Persisted-feature checklist

Complete these items in one pull request:

1. Increase each changed module version and add its forward migration. Keep
   released migrations unchanged.
2. Update `catalog.rs` with the current version, object registry, and accepted
   shapes, including supported historical shapes.
3. Update preflight and final invariants for the changed format. Preflight
   rejects unknown or newer state before the first write.
4. Register new persisted modules and add them to the order in `migration.rs`.
5. Test through `prepare`, update only the candidate fixture, and follow the
   [fixture guide](../../apps/desktop/src-tauri/tests/fixtures/releases/README.md).
6. Run the fixture check, database compatibility test, and `bun run quality`.

The repository keeps one sanitized SQLite fixture for every published stable release and one fixture for the next release candidate. Official fixture database bytes are immutable. The release test copies each fixture, opens it with production migration code, checks every retained provider row and visible product state, and proves an idempotent reopen. The release gate requires the official fixture set to match the published stable GitHub Release set. It runs the test with the exact candidate commit and includes a per-fixture result in its machine-readable evidence.

This contract adds fixture maintenance and strict schema checks to each release. It also makes a format change visible and reviewable. In return, every later release has a permanent upgrade path from every official release, and downgrade attempts fail without silent data loss.
