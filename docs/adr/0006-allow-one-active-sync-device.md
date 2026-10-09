# Allow one active synchronization device

Each Tokenmaxxer Profile has one Active Mac authorized to synchronize usage. Restoring the Profile on another Mac transfers write authority and invalidates the prior session, avoiding double-counting and conflict resolution while deliberately excluding concurrent multi-device use from the MVP.

Routine usage and Provider Enablement synchronization update the Profile's
`lastSyncedAt`. They do not rewrite the device authority document to record
the same contact. `lastSeenAt` records device provisioning contact; it is not a
usage freshness or online-status signal. First-backfill completion and Active
Mac transfer still update device authority when their rules require it. This
keeps diagnostic authority reads independent of routine device heartbeat
writes without a new table or data migration.
