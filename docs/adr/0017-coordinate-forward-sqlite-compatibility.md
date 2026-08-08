---
status: accepted
---

# Coordinate Forward SQLite Compatibility

One Rust database coordinator owns SQLite readiness. It first inspects the global format, every registered module version, and the known object shapes without a write. It rejects an unknown or newer format before it creates a backup or starts a migration.

For a known older format, the coordinator creates and verifies one durable full-database backup. A durable marker identifies a migration retry. Before a later migration starts, the coordinator atomically replaces the completed migration's backup with the latest source database. It then runs registered module migrations in a fixed order and checks the complete structural and domain invariants. A failure keeps the backup, stops startup, and does not delete, reset, or partly accept the database. A successful open removes the marker and returns an opaque Ready token. Native persistence, provider work, synchronization, and update checks require this token.

Every persisted module has an explicit current schema version. A current release increases a module version when its format changes. This lets an older release detect a database from a newer release and fail closed without a write. Legacy column inspection can identify only listed historical formats. It cannot infer an unknown format.

The repository keeps one sanitized SQLite fixture for every published stable release and one fixture for the next release candidate. Official fixture database bytes are immutable. The release test copies each fixture, opens it with production migration code, checks every retained provider row and visible product state, and proves an idempotent reopen. The release gate requires the official fixture set to match the published stable GitHub Release set. It runs the test with the exact candidate commit and includes a per-fixture result in its machine-readable evidence.

This contract adds fixture maintenance and strict schema checks to each release. It also makes a format change visible and reviewable. In return, every later release has a permanent upgrade path from every official release, and downgrade attempts fail without silent data loss.
