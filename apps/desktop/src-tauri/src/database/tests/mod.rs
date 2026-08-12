mod release_compatibility;

use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use rusqlite::{Connection, OpenFlags, params};

use super::{
    DatabaseOpenError,
    catalog::{DATABASE_FORMAT_VERSION, MODULES, STRICT_TABLES, normalize_sql},
    invariants::verify_invariants,
    migration::{
        PrepareFault, coordinator_backup_partial_path, coordinator_backup_path,
        validate_coordinator_backup,
    },
    prepare, prepare_with_fault,
};
use crate::{providers, sanitized, updater};

static NEXT_DATABASE: AtomicU64 = AtomicU64::new(1);

struct TestDatabase(PathBuf);

impl TestDatabase {
    fn new() -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        let id = NEXT_DATABASE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "touchgrassbar-database-coordinator-{}-{timestamp}-{id}.sqlite3",
            std::process::id()
        ));
        Self(path)
    }

    fn make_ready_source_look_legacy(&self) {
        prepare(&self.0).expect("prepare source database");
        let connection = Connection::open(&self.0).expect("open source database");
        connection
            .execute_batch(
                "DELETE FROM touchgrassbar_schema_versions
                 WHERE module = 'database-coordinator';
                 PRAGMA user_version = 5;
                 PRAGMA journal_mode = DELETE;",
            )
            .expect("make legacy source");
    }
}

impl Drop for TestDatabase {
    fn drop(&mut self) {
        let parent = self.0.parent().expect("temporary parent");
        let stem = self
            .0
            .file_name()
            .and_then(|name| name.to_str())
            .expect("temporary file name");
        if let Ok(entries) = fs::read_dir(parent) {
            for entry in entries.flatten() {
                if entry.file_name().to_string_lossy().starts_with(stem) {
                    let _ = fs::remove_file(entry.path());
                }
            }
        }
    }
}

#[test]
fn prepares_one_complete_versioned_database() {
    let database = TestDatabase::new();

    let prepared = prepare(&database.0).expect("prepare database");

    assert_eq!(prepared.path(), database.0.as_path());
    let connection = Connection::open(&database.0).expect("open prepared database");
    let versions = connection
        .prepare("SELECT module, version FROM touchgrassbar_schema_versions ORDER BY module")
        .expect("prepare version query")
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .expect("query versions")
        .collect::<Result<Vec<_>, _>>()
        .expect("read versions");
    assert_eq!(
        versions,
        vec![
            ("claude-usage-index".to_owned(), 7),
            ("codex-usage-index".to_owned(), 7),
            ("database-coordinator".to_owned(), 1),
            ("desktop-lifecycle".to_owned(), 5),
            ("sanitized-desktop-state".to_owned(), 7),
            ("update-state".to_owned(), 3),
        ]
    );
    assert_eq!(
        connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .expect("database format"),
        DATABASE_FORMAT_VERSION
    );
}

#[test]
fn coordinator_upgrades_the_codex_v6_file_turn_shape_with_daily_references() {
    let database = TestDatabase::new();
    prepare(&database.0).expect("prepare current database");
    let connection = Connection::open(&database.0).expect("open current database");
    connection
        .pragma_update(None, "foreign_keys", false)
        .expect("disable foreign keys for historical fixture shape");
    connection
        .execute_batch(
            "DROP INDEX codex_usage_file_turns_by_turn_id;
             ALTER TABLE codex_usage_file_turns RENAME TO codex_usage_file_turns_v7;
             CREATE TABLE codex_usage_file_turns (
               path TEXT NOT NULL,
               turn_id TEXT NOT NULL,
               PRIMARY KEY (path, turn_id),
               FOREIGN KEY(path) REFERENCES codex_usage_files(path) ON DELETE CASCADE
             );
             DROP TABLE codex_usage_file_turns_v7;
             CREATE INDEX codex_usage_file_turns_by_turn_id
             ON codex_usage_file_turns(turn_id);
             INSERT INTO codex_usage_files(
               path, file_identity, size_bytes, modified_ns, parsed_offset,
               parser_version, completion_state, active_turn_id, schema_supported
             ) VALUES(
               'private-rollout', '1:2', 10, 20, 10, 15, 'complete',
               'private-turn', 1
             );
             INSERT INTO codex_usage_fast_turns(turn_id, model)
             VALUES('private-turn', 'gpt-5.6-sol');
             INSERT INTO codex_usage_file_turns(path, turn_id)
             VALUES('private-rollout', 'private-turn');
             UPDATE touchgrassbar_schema_versions SET version = 6
             WHERE module = 'codex-usage-index';
             PRAGMA journal_mode = DELETE;",
        )
        .expect("make a known Codex v6 source");
    drop(connection);

    prepare(&database.0).expect("upgrade the Codex v6 source");

    let connection = Connection::open(&database.0).expect("open upgraded database");
    assert_eq!(
        providers::codex_usage_schema_version(&connection).expect("Codex schema version"),
        providers::CODEX_USAGE_SCHEMA_VERSION
    );
    let columns = connection
        .prepare("SELECT name FROM pragma_table_info('codex_usage_file_turns') ORDER BY cid")
        .expect("prepare file-turn columns")
        .query_map([], |row| row.get::<_, String>(0))
        .expect("query file-turn columns")
        .collect::<Result<Vec<_>, _>>()
        .expect("read file-turn columns");
    assert_eq!(columns, ["path", "turn_id", "day"]);
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM codex_usage_files", [], |row| {
                row.get::<_, i64>(0)
            })
            .expect("count retained Codex files"),
        1
    );
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM codex_usage_file_turns", [], |row| {
                row.get::<_, i64>(0)
            })
            .expect("count rebuilt file-turn references"),
        0
    );
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM codex_usage_fast_turns", [], |row| {
                row.get::<_, i64>(0)
            })
            .expect("count cleared Fast details"),
        0
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT active_turn_id FROM codex_usage_files WHERE path = 'private-rollout'",
                [],
                |row| row.get::<_, Option<String>>(0)
            )
            .expect("read cleared active turn"),
        None
    );
    verify_invariants(&connection).expect("verify upgraded database");
    assert!(coordinator_backup_path(&database.0).is_file());
}

#[test]
fn coordinator_adds_the_v7_profile_completion_field_without_losing_activation() {
    let database = TestDatabase::new();
    prepare(&database.0).expect("prepare current database");
    let connection = Connection::open(&database.0).expect("open current database");
    connection
        .execute_batch(
            "INSERT INTO usage_sync_generations(active_generation, queue_state)
               VALUES(1, 'active');
             INSERT INTO usage_sync_generation_activations(
               active_generation, ranking_day, activated_at
             ) VALUES(1, '2026-08-08', 1786147200000);
             PRAGMA foreign_keys = OFF;
             ALTER TABLE usage_sync_generation_activations
               RENAME TO usage_sync_generation_activations_v7;
             CREATE TABLE usage_sync_generation_activations (
               active_generation INTEGER PRIMARY KEY,
               ranking_day TEXT NOT NULL CHECK(length(ranking_day) = 10),
               activated_at INTEGER NOT NULL
                 CHECK(activated_at >= 0 AND activated_at <= 9007199254740991),
               FOREIGN KEY(active_generation)
                 REFERENCES usage_sync_generations(active_generation)
             ) STRICT;
             INSERT INTO usage_sync_generation_activations(
               active_generation, ranking_day, activated_at
             ) SELECT active_generation, ranking_day, activated_at
               FROM usage_sync_generation_activations_v7;
             DROP TABLE usage_sync_generation_activations_v7;
             ALTER TABLE sanitized_desktop_state
               RENAME TO sanitized_desktop_state_v7;
             CREATE TABLE sanitized_desktop_state (
               singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
               schema_version INTEGER NOT NULL CHECK (schema_version = 6),
               contract_version INTEGER NOT NULL CHECK (contract_version = 4),
               revision TEXT NOT NULL CHECK (
                 length(revision) > 0 AND revision NOT GLOB '*[^0-9]*'
               ),
               snapshot_json TEXT NOT NULL
             );
             INSERT INTO sanitized_desktop_state(
               singleton, schema_version, contract_version, revision, snapshot_json
             ) SELECT singleton, 6, contract_version, revision, snapshot_json
               FROM sanitized_desktop_state_v7;
             DROP TABLE sanitized_desktop_state_v7;
             UPDATE touchgrassbar_schema_versions SET version = 6
             WHERE module = 'sanitized-desktop-state';
             PRAGMA journal_mode = DELETE;",
        )
        .expect("make a known read-model v6 source");
    drop(connection);

    prepare(&database.0).expect("upgrade the read-model v6 source");

    let connection = Connection::open(&database.0).expect("open upgraded database");
    assert_eq!(
        sanitized::read_model_schema_version(&connection).unwrap(),
        7
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT activated_at, profile_backfill_completed
                 FROM usage_sync_generation_activations WHERE active_generation = 1",
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .expect("read retained activation"),
        (1_786_147_200_000, 0)
    );
    drop(connection);
    let before = fs::read(&database.0).expect("read migrated database");
    prepare(&database.0).expect("reopen migrated database");
    assert_eq!(
        fs::read(&database.0).expect("reread migrated database"),
        before
    );
}

#[test]
fn prepares_strict_usage_sync_tables_and_cascade_carryovers() {
    let database = TestDatabase::new();
    prepare(&database.0).expect("prepare database");
    let connection = Connection::open(&database.0).expect("open prepared database");
    connection
        .pragma_update(None, "foreign_keys", true)
        .expect("enable foreign keys");

    for table in STRICT_TABLES {
        let schema = connection
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE type = 'table' AND name = ?1",
                [table],
                |row| row.get::<_, String>(0),
            )
            .expect("read strict table schema");
        assert!(normalize_sql(&schema).ends_with(")strict"), "{table}");
    }

    connection
        .execute_batch(
            "INSERT INTO usage_sync_generations(active_generation, queue_state)
               VALUES(1, 'abandoned');
             INSERT INTO usage_sync_latest_outbox(
               active_generation, provider, ranking_day, revision,
               snapshot_json, queue_state
             ) VALUES(1, 'codex', '2026-08-11', 1, '{}', 'abandoned');
             INSERT INTO usage_sync_transfer_day_carryovers(
               active_generation, provider, ranking_day, carryover_kind
             ) VALUES(1, 'codex', '2026-08-11', 'pending-segment');
             DELETE FROM usage_sync_latest_outbox
             WHERE active_generation = 1 AND provider = 'codex'
               AND ranking_day = '2026-08-11';",
        )
        .expect("exercise carryover cascade");
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM usage_sync_transfer_day_carryovers",
                [],
                |row| row.get::<_, i64>(0),
            )
            .expect("count carryovers"),
        0
    );
}

#[test]
fn rejects_two_live_usage_sync_generations() {
    let database = TestDatabase::new();
    prepare(&database.0).expect("prepare database");
    let connection = Connection::open(&database.0).expect("open prepared database");
    connection
        .execute_batch(
            "INSERT INTO usage_sync_generations(active_generation, queue_state)
               VALUES(1, 'active'), (2, 'blocked');",
        )
        .expect("write invalid usage sync generations");

    assert_eq!(
        verify_invariants(&connection).expect_err("reject two live generations"),
        DatabaseOpenError::InvariantFailed {
            invariant: "usage-sync-values"
        }
    );
}

#[test]
fn rejects_a_future_database_before_writing() {
    let database = TestDatabase::new();
    let connection = Connection::open(&database.0).expect("create future database");
    connection
        .execute_batch(
            "CREATE TABLE touchgrassbar_schema_versions (
               module TEXT PRIMARY KEY,
               version INTEGER NOT NULL CHECK (version >= 1)
             );
             INSERT INTO touchgrassbar_schema_versions(module, version)
             VALUES ('database-coordinator', 2);",
        )
        .expect("write future version");
    drop(connection);
    let before = fs::read(&database.0).expect("read future database");

    assert!(matches!(
        prepare(&database.0),
        Err(DatabaseOpenError::UnsupportedFuture {
            module: "database-coordinator"
        })
    ));
    assert_eq!(
        fs::read(&database.0).expect("reread future database"),
        before
    );
}

#[test]
fn rejects_every_future_module_and_format_without_writing() {
    for (module, current) in MODULES {
        let database = TestDatabase::new();
        let connection = Connection::open(&database.0).expect("create future database");
        connection
            .execute_batch(
                "CREATE TABLE touchgrassbar_schema_versions (
                   module TEXT PRIMARY KEY,
                   version INTEGER NOT NULL CHECK (version >= 1)
                 );",
            )
            .expect("create version table");
        connection
            .execute(
                "INSERT INTO touchgrassbar_schema_versions(module, version) VALUES(?1, ?2)",
                params![module, current + 1],
            )
            .expect("write future module");
        drop(connection);
        let before = fs::read(&database.0).expect("read future database");

        assert_eq!(
            prepare(&database.0).expect_err("reject future module"),
            DatabaseOpenError::UnsupportedFuture { module }
        );
        assert_eq!(fs::read(&database.0).expect("reread database"), before);
    }

    let database = TestDatabase::new();
    let connection = Connection::open(&database.0).expect("create future database");
    connection
        .pragma_update(None, "user_version", DATABASE_FORMAT_VERSION + 1)
        .expect("write future format");
    drop(connection);
    let before = fs::read(&database.0).expect("read future database");
    assert_eq!(
        prepare(&database.0).expect_err("reject future format"),
        DatabaseOpenError::UnsupportedFuture {
            module: "database-format"
        }
    );
    assert_eq!(fs::read(&database.0).expect("reread database"), before);
}

#[test]
fn rejects_unregistered_modules_and_objects() {
    let module_database = TestDatabase::new();
    let connection = Connection::open(&module_database.0).expect("create unknown module");
    connection
        .execute_batch(
            "CREATE TABLE touchgrassbar_schema_versions (
               module TEXT PRIMARY KEY,
               version INTEGER NOT NULL CHECK (version >= 1)
             );
             INSERT INTO touchgrassbar_schema_versions(module, version)
             VALUES ('future-private-state', 1);",
        )
        .expect("write unknown module");
    drop(connection);
    assert_eq!(
        prepare(&module_database.0).expect_err("reject unknown module"),
        DatabaseOpenError::UnsupportedFuture {
            module: "unregistered-module"
        }
    );

    let object_database = TestDatabase::new();
    Connection::open(&object_database.0)
        .expect("create unknown object")
        .execute("CREATE TABLE future_private_state(value TEXT)", [])
        .expect("write unknown object");
    assert_eq!(
        prepare(&object_database.0).expect_err("reject unknown object"),
        DatabaseOpenError::UnsupportedFuture {
            module: "unregistered-object"
        }
    );
}

#[test]
fn inspects_every_legacy_module_before_any_migration_write() {
    let database = TestDatabase::new();
    Connection::open(&database.0)
        .expect("create unknown legacy shape")
        .execute_batch(
            "CREATE TABLE touchgrassbar_update_state (
               singleton INTEGER PRIMARY KEY,
               last_automatic_check_at INTEGER,
               offered_version TEXT,
               minimum_required_version TEXT,
               future_private_value TEXT
             );
             INSERT INTO touchgrassbar_update_state VALUES(1, 10, NULL, NULL, 'kept');",
        )
        .expect("write unknown legacy shape");
    let before = fs::read(&database.0).expect("read source database");

    assert_eq!(
        prepare(&database.0).expect_err("reject unknown legacy shape"),
        DatabaseOpenError::MigrationFailed {
            stage: "inspect-update-state"
        }
    );
    assert_eq!(fs::read(&database.0).expect("reread source"), before);
    assert!(!coordinator_backup_path(&database.0).exists());
}

#[test]
fn rejects_an_unknown_registered_module_shape_before_backup_or_write() {
    let database = TestDatabase::new();
    database.make_ready_source_look_legacy();
    Connection::open(&database.0)
        .expect("open source database")
        .execute(
            "ALTER TABLE codex_usage_files ADD COLUMN future_private_value TEXT",
            [],
        )
        .expect("add unknown column");
    let before = fs::read(&database.0).expect("read source database");

    assert_eq!(
        prepare(&database.0).expect_err("reject unknown module shape"),
        DatabaseOpenError::MigrationFailed {
            stage: "inspect-table-columns"
        }
    );
    assert_eq!(fs::read(&database.0).expect("reread source"), before);
    assert!(!coordinator_backup_path(&database.0).exists());
}

#[test]
fn rejects_unknown_legacy_definitions_before_backup_or_write() {
    let mutations = [
        (
            "index",
            "DROP INDEX codex_usage_model_days_by_day;
             CREATE INDEX codex_usage_model_days_by_day
             ON codex_usage_file_model_days(model);",
        ),
        (
            "missing required index",
            "DROP INDEX codex_usage_model_days_by_day;",
        ),
        (
            "primary key",
            "ALTER TABLE codex_account_usage_days
               RENAME TO codex_account_usage_days_old;
             CREATE TABLE codex_account_usage_days (
               day TEXT NOT NULL,
               tokens INTEGER PRIMARY KEY NOT NULL
             );
             INSERT INTO codex_account_usage_days(day, tokens)
               SELECT day, tokens FROM codex_account_usage_days_old;
             DROP TABLE codex_account_usage_days_old;",
        ),
        (
            "check constraint",
            "ALTER TABLE provider_settings RENAME TO provider_settings_old;
             CREATE TABLE provider_settings (
               provider TEXT PRIMARY KEY CHECK (
                 provider IN ('codex', 'claude', 'future')
               ),
               enabled INTEGER NOT NULL CHECK (enabled IN (0, 1))
             );
             INSERT INTO provider_settings(provider, enabled)
               SELECT provider, enabled FROM provider_settings_old;
             DROP TABLE provider_settings_old;",
        ),
        (
            "single-quoted literal case",
            "PRAGMA writable_schema = ON;
             UPDATE sqlite_schema
             SET sql = replace(
               sql,
               '''codex'', ''claude''',
               '''CODEX'', ''CLAUDE'''
             )
             WHERE type = 'table' AND name = 'provider_settings';
             PRAGMA writable_schema = OFF;",
        ),
        (
            "single-quoted literal whitespace",
            "PRAGMA writable_schema = ON;
             UPDATE sqlite_schema
             SET sql = replace(
               sql,
               '''codex'', ''claude''',
               '''cod ex'', ''claude'''
             )
             WHERE type = 'table' AND name = 'provider_settings';
             PRAGMA writable_schema = OFF;",
        ),
        (
            "default",
            "ALTER TABLE provider_settings RENAME TO provider_settings_old;
             CREATE TABLE provider_settings (
               provider TEXT PRIMARY KEY CHECK (provider IN ('codex', 'claude')),
               enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1))
             );
             INSERT INTO provider_settings(provider, enabled)
               SELECT provider, enabled FROM provider_settings_old;
             DROP TABLE provider_settings_old;",
        ),
        (
            "foreign key",
            "ALTER TABLE codex_usage_file_days
               RENAME TO codex_usage_file_days_old;
             CREATE TABLE codex_usage_file_days (
               path TEXT NOT NULL,
               day TEXT NOT NULL,
               observed_tokens INTEGER NOT NULL,
               priced_tokens INTEGER NOT NULL,
               cost_usd REAL NOT NULL,
               complete INTEGER NOT NULL,
               observed_through TEXT NOT NULL,
               priced_observed_through TEXT,
               pricing_fingerprint TEXT,
               PRIMARY KEY (path, day)
             );
             INSERT INTO codex_usage_file_days
               SELECT * FROM codex_usage_file_days_old;
             DROP TABLE codex_usage_file_days_old;",
        ),
        (
            "definition from the wrong module version",
            "ALTER TABLE sanitized_desktop_state
               RENAME TO sanitized_desktop_state_old;
             CREATE TABLE sanitized_desktop_state (
               singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
               schema_version INTEGER NOT NULL CHECK (schema_version = 4),
               contract_version INTEGER NOT NULL CHECK (contract_version = 3),
               revision TEXT NOT NULL CHECK (
                 length(revision) > 0 AND revision NOT GLOB '*[^0-9]*'
               ),
               snapshot_json TEXT NOT NULL
             );
             INSERT INTO sanitized_desktop_state
               SELECT singleton, 4, 3, revision, snapshot_json
               FROM sanitized_desktop_state_old;
             DROP TABLE sanitized_desktop_state_old;",
        ),
        (
            "view",
            "DROP VIEW touchgrassbar_update_state;
             CREATE VIEW touchgrassbar_update_state AS
             SELECT singleton, automatic_checks_enabled, last_automatic_check_at,
                    offered_version, minimum_required_version
             FROM touchgrassbar_update_state_v3
             WHERE singleton = 1;",
        ),
        (
            "trigger",
            "CREATE TRIGGER lifecycle_state_future_trigger
             AFTER UPDATE ON lifecycle_state
             BEGIN
               SELECT 1;
             END;",
        ),
    ];

    for (definition, mutation) in mutations {
        let database = TestDatabase::new();
        database.make_ready_source_look_legacy();
        Connection::open(&database.0)
            .expect("open legacy database")
            .execute_batch(mutation)
            .unwrap_or_else(|error| panic!("mutate {definition} definition: {error}"));
        let before = fs::read(&database.0).expect("read changed legacy database");

        assert!(
            prepare(&database.0).is_err(),
            "accepts unknown legacy {definition} definition"
        );
        assert!(
            fs::read(&database.0).expect("reread changed legacy database") == before,
            "writes before rejecting unknown legacy {definition} definition"
        );
        assert!(
            !coordinator_backup_path(&database.0).exists(),
            "creates a backup before rejecting unknown legacy {definition} definition"
        );
    }
}

#[test]
fn a_ready_reopen_is_byte_and_backup_idempotent() {
    let database = TestDatabase::new();
    prepare(&database.0).expect("first prepare");
    let before = fs::read(&database.0).expect("read ready database");
    let backup_before = coordinator_backup_path(&database.0).exists();

    prepare(&database.0).expect("second prepare");

    assert_eq!(fs::read(&database.0).expect("reread database"), before);
    assert_eq!(coordinator_backup_path(&database.0).exists(), backup_before);
}

#[test]
fn ready_database_rejects_unversioned_structural_changes() {
    let database = TestDatabase::new();
    prepare(&database.0).expect("prepare database");
    Connection::open(&database.0)
        .expect("open database")
        .execute(
            "ALTER TABLE touchgrassbar_update_state_v3 ADD COLUMN future_value TEXT",
            [],
        )
        .expect("add unversioned column");

    assert_eq!(
        prepare(&database.0).expect_err("reject changed structure"),
        DatabaseOpenError::MigrationFailed {
            stage: "inspect-update-state"
        }
    );
}

#[test]
fn every_released_module_fails_closed_on_the_current_database() {
    const {
        assert!(DATABASE_FORMAT_VERSION > 5);
        assert!(sanitized::READ_MODEL_SCHEMA_VERSION > 4);
        assert!(providers::CODEX_USAGE_SCHEMA_VERSION > 2);
        assert!(providers::CLAUDE_USAGE_SCHEMA_VERSION > 3);
        assert!(updater::DATABASE_SCHEMA_VERSION > 2);
    }

    let database = TestDatabase::new();
    prepare(&database.0).expect("prepare current database");
    let database_before = fs::read(&database.0).expect("read current database");
    let connection = Connection::open(&database.0).expect("open as released updater");
    let before = connection
        .query_row(
            "SELECT automatic_checks_enabled, last_automatic_check_at,
                    offered_version, minimum_required_version
             FROM touchgrassbar_update_state_v3 WHERE singleton = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            },
        )
        .expect("read current update state");

    connection
        .execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = FULL;
             CREATE TABLE IF NOT EXISTS touchgrassbar_update_state (
               singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
               automatic_checks_enabled INTEGER NOT NULL DEFAULT 1,
               last_automatic_check_at INTEGER,
               offered_version TEXT,
               minimum_required_version TEXT
             );",
        )
        .expect("released updater inspection remains read-only");
    assert!(
        connection
            .execute(
                "INSERT OR IGNORE INTO touchgrassbar_update_state (
                   singleton, automatic_checks_enabled, last_automatic_check_at,
                   offered_version, minimum_required_version
                 ) VALUES (1, 1, NULL, NULL, NULL)",
                [],
            )
            .is_err(),
        "the compatibility view must reject a released updater write"
    );
    let after = connection
        .query_row(
            "SELECT automatic_checks_enabled, last_automatic_check_at,
                    offered_version, minimum_required_version
             FROM touchgrassbar_update_state_v3 WHERE singleton = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            },
        )
        .expect("reread current update state");
    assert_eq!(after, before);
    drop(connection);
    assert_eq!(
        fs::read(&database.0).expect("reread current database"),
        database_before,
        "the released updater must not change the current database"
    );
}

#[test]
fn migration_faults_keep_a_recoverable_backup_and_resume() {
    for fault in [
        PrepareFault::BeforeBackupComplete,
        PrepareFault::AfterBackupComplete,
        PrepareFault::BeforeFinalCommit,
        PrepareFault::AfterCommitBeforeReady,
    ] {
        let database = TestDatabase::new();
        database.make_ready_source_look_legacy();
        let source_before = fs::read(&database.0).expect("read legacy source");

        assert!(prepare_with_fault(&database.0, fault).is_err());
        if fault == PrepareFault::BeforeBackupComplete {
            assert_eq!(
                fs::read(&database.0).expect("read unchanged source"),
                source_before
            );
            assert!(coordinator_backup_partial_path(&database.0).exists());
        } else {
            assert!(coordinator_backup_path(&database.0).exists());
            validate_coordinator_backup(&coordinator_backup_path(&database.0))
                .expect("valid retained backup");
        }

        prepare(&database.0).expect("resume migration");
        assert!(coordinator_backup_path(&database.0).exists());
        let connection = Connection::open(&database.0).expect("open resumed database");
        verify_invariants(&connection).expect("resumed invariants");
    }
}

#[test]
fn a_later_migration_replaces_the_previous_successful_backup() {
    let database = TestDatabase::new();
    database.make_ready_source_look_legacy();
    prepare(&database.0).expect("complete first migration");
    Connection::open(&database.0)
        .expect("open current database")
        .execute(
            "UPDATE touchgrassbar_update_state_v3
             SET last_automatic_check_at = 77 WHERE singleton = 1",
            [],
        )
        .expect("write newer durable state");
    database.make_ready_source_look_legacy();

    prepare(&database.0).expect("complete later migration");

    let backup = Connection::open_with_flags(
        coordinator_backup_path(&database.0),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("open latest source backup");
    assert_eq!(
        backup
            .query_row(
                "SELECT last_automatic_check_at
                 FROM touchgrassbar_update_state_v3 WHERE singleton = 1",
                [],
                |row| row.get::<_, Option<i64>>(0),
            )
            .expect("read backed-up state"),
        Some(77)
    );
}

#[test]
fn ready_database_fails_closed_when_its_recovery_backup_is_damaged() {
    let database = TestDatabase::new();
    database.make_ready_source_look_legacy();
    prepare(&database.0).expect("migrate legacy database");
    let before = fs::read(&database.0).expect("read ready database");
    fs::write(coordinator_backup_path(&database.0), b"damaged backup").expect("damage backup");

    assert!(matches!(
        prepare(&database.0),
        Err(DatabaseOpenError::MigrationFailed { .. })
    ));
    assert_eq!(fs::read(&database.0).expect("reread database"), before);
    assert_eq!(
        fs::read(coordinator_backup_path(&database.0)).expect("read damaged backup"),
        b"damaged backup"
    );
}

#[test]
fn invariant_failure_stays_closed_and_keeps_the_source_backup() {
    let database = TestDatabase::new();
    database.make_ready_source_look_legacy();
    let connection = Connection::open(&database.0).expect("open legacy database");
    connection
        .execute(
            "UPDATE sanitized_desktop_state SET snapshot_json = 'private-value'",
            [],
        )
        .expect("damage sanitized state");
    drop(connection);

    assert_eq!(
        prepare(&database.0).expect_err("reject damaged state"),
        DatabaseOpenError::MigrationFailed {
            stage: "sanitized-desktop-state"
        }
    );
    assert!(coordinator_backup_path(&database.0).exists());
    validate_coordinator_backup(&coordinator_backup_path(&database.0))
        .expect("failed migration retains the source backup");
}

#[test]
fn accepts_the_full_sixty_day_claude_history_window() {
    let database = TestDatabase::new();
    prepare(&database.0).expect("prepare database");
    Connection::open(&database.0)
        .expect("open database")
        .execute_batch(
            "INSERT INTO claude_usage_daily(
               day, observed_tokens, coverage, observed_through, revision, priced_tokens
             ) VALUES
               ('2026-01-01', 10, 'complete', '2026-01-01T00:00:00Z', 1, 0),
               ('2026-03-01', 20, 'complete', '2026-03-01T00:00:00Z', 1, 0);",
        )
        .expect("write sixty-day history");

    prepare(&database.0).expect("accept sixty-day Claude history");
}

#[test]
fn accepts_sixty_day_codex_aggregates_with_thirty_day_cost_detail() {
    let database = TestDatabase::new();
    prepare(&database.0).expect("prepare database");
    Connection::open(&database.0)
        .expect("open database")
        .execute_batch(
            "INSERT INTO codex_usage_files(
               path, file_identity, size_bytes, modified_ns, parsed_offset,
               parser_version, completion_state, schema_supported,
               accounting_ready
             ) VALUES(
               'fixture.jsonl', 'fixture', 0, 0, 0,
               15, 'complete', 1, 1
             );
             INSERT INTO codex_usage_file_days(
               path, day, observed_tokens, priced_tokens, cost_usd, complete,
               observed_through, priced_observed_through, pricing_fingerprint
             ) VALUES
               ('fixture.jsonl', '2026-01-01', 10, 0, 0.0, 0,
                '2026-01-01T00:00:00Z', NULL, NULL),
               ('fixture.jsonl', '2026-03-01', 20, 0, 0.0, 0,
                '2026-03-01T00:00:00Z', NULL, NULL);
             INSERT INTO codex_usage_file_model_days(
               path, day, model, pricing_input_tokens, pricing_mode,
               input_tokens, cached_input_tokens, cache_write_input_tokens,
               output_tokens, reasoning_output_tokens, observed_tokens,
               cost_usd, pricing_basis, pricing_fingerprint, complete,
               observed_through
             ) VALUES(
               'fixture.jsonl', '2026-02-01', 'gpt-5.6-sol', 0, 'standard',
               0, 0, 0, 10, 0, 10, 0.1, 'fixture-v1', 'fixture', 1,
               '2026-02-01T00:00:00Z'
             );",
        )
        .expect("write retained Codex history");

    prepare(&database.0).expect("accept retained Codex history");
}

#[test]
fn rejects_non_calendar_ranking_days() {
    let database = TestDatabase::new();
    prepare(&database.0).expect("prepare database");
    Connection::open(&database.0)
        .expect("open database")
        .execute(
            "INSERT INTO codex_account_usage_days(day, tokens) VALUES('2026-02-30', 1)",
            [],
        )
        .expect("write invalid day");

    assert_eq!(
        prepare(&database.0).expect_err("reject invalid day"),
        DatabaseOpenError::InvariantFailed {
            invariant: "codex-usage-values"
        }
    );
}

#[test]
fn rejects_a_correction_edge_without_its_superseded_message() {
    let database = TestDatabase::new();
    prepare(&database.0).expect("prepare database");
    Connection::open(&database.0)
        .expect("open database")
        .execute_batch(
            "INSERT INTO claude_usage_frames(frame_key, day, observed_at, parser_version)
             VALUES('replacement', '2026-01-01', '2026-01-01T00:00:00Z', 1);
             INSERT INTO claude_usage_message_supersedes(
               replacement_frame_key, superseded_frame_key, parser_version
             ) VALUES('replacement', 'missing', 1);",
        )
        .expect("write incomplete correction edge");

    assert_eq!(
        prepare(&database.0).expect_err("reject incomplete correction edge"),
        DatabaseOpenError::InvariantFailed {
            invariant: "claude-correction-edges"
        }
    );
}

#[test]
fn rejects_an_index_name_with_the_wrong_definition() {
    let database = TestDatabase::new();
    prepare(&database.0).expect("prepare database");
    Connection::open(&database.0)
        .expect("open database")
        .execute_batch(
            "DROP INDEX codex_usage_model_days_by_day;
             CREATE INDEX codex_usage_model_days_by_day
             ON codex_usage_file_model_days(model);",
        )
        .expect("replace index definition");

    assert_eq!(
        prepare(&database.0).expect_err("reject changed index definition"),
        DatabaseOpenError::MigrationFailed {
            stage: "inspect-object-definitions"
        }
    );
}

#[test]
fn rejects_a_double_quoted_constraint_literal_before_backup_or_write() {
    let database = TestDatabase::new();
    database.make_ready_source_look_legacy();
    let connection = Connection::open(&database.0).expect("open legacy database");
    connection
        .execute_batch(
            r#"PRAGMA writable_schema = ON;
               UPDATE sqlite_schema
               SET sql = replace(
                 sql,
                 'BETWEEN 1 AND 64',
                 'BETWEEN "1" AND 64'
               )
               WHERE type = 'table'
                 AND name = 'touchgrassbar_update_state_v3';
               PRAGMA writable_schema = OFF;"#,
        )
        .expect("change constraint literal");
    let changed_definition = connection
        .query_row(
            "SELECT sql FROM sqlite_schema
             WHERE type = 'table' AND name = 'touchgrassbar_update_state_v3'",
            [],
            |row| row.get::<_, String>(0),
        )
        .expect("read changed definition");
    assert!(changed_definition.contains(r#"BETWEEN "1" AND 64"#));
    drop(connection);
    let before = fs::read(&database.0).expect("read changed legacy database");

    assert_eq!(
        prepare(&database.0).expect_err("reject double-quoted constraint literal"),
        DatabaseOpenError::MigrationFailed {
            stage: "inspect-object-definitions"
        }
    );
    assert!(
        fs::read(&database.0).expect("reread changed legacy database") == before,
        "writes before rejecting a double-quoted constraint literal"
    );
    assert!(!coordinator_backup_path(&database.0).exists());
}

#[test]
fn normalizes_sql_syntax_without_changing_quoted_bytes() {
    assert_eq!(
        normalize_sql(
            r#"CREATE TABLE "Touch""Grass" (
                 value TEXT CHECK (value IN ('CODE X', 'it''s'))
               )"#,
        ),
        r#"createtable"Touch""Grass"(valuetextcheck(valuein('CODE X','it''s')))"#
    );
    assert_ne!(
        normalize_sql(r#"CREATE TABLE "touchgrassbar_update_state_v3" (singleton INTEGER)"#),
        normalize_sql("CREATE TABLE touchgrassbar_update_state_v3(singleton INTEGER)")
    );
    assert_eq!(
        normalize_sql(r#"CHECK (value BETWEEN "1" AND 64)"#),
        r#"check(valuebetween"1"and64)"#
    );
    assert_ne!(
        normalize_sql("CHECK (provider IN ('CODEX', 'CLAUDE'))"),
        normalize_sql("CHECK (provider IN ('codex', 'claude'))")
    );
    assert_ne!(
        normalize_sql("CHECK (provider IN ('cod ex', 'claude'))"),
        normalize_sql("CHECK (provider IN ('codex', 'claude'))")
    );
}

#[test]
fn diagnostics_are_bounded_and_do_not_expose_values() {
    for error in [
        DatabaseOpenError::UnsupportedFuture {
            module: "database-format",
        },
        DatabaseOpenError::MigrationFailed {
            stage: "desktop-lifecycle",
        },
        DatabaseOpenError::InvariantFailed {
            invariant: "sanitized-state",
        },
    ] {
        let diagnostic = format!("{}:{}", error.diagnostic(), error.detail());
        assert!(diagnostic.len() <= 80);
        assert!(!diagnostic.contains('/'));
        assert!(!diagnostic.contains("private-value"));
    }
}
