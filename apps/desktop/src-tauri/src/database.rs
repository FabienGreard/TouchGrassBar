use std::{
    fs,
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
    time::Duration,
};

use rusqlite::{Connection, OpenFlags, params};

use crate::{lifecycle, providers, sanitized, updater};

const DATABASE_FORMAT_VERSION: i64 = 6;
const COORDINATOR_SCHEMA_MODULE: &str = "database-coordinator";
const COORDINATOR_SCHEMA_VERSION: i64 = 1;

const MODULES: &[(&str, i64)] = &[
    (
        lifecycle::DATABASE_SCHEMA_MODULE,
        lifecycle::DATABASE_SCHEMA_VERSION,
    ),
    (
        sanitized::READ_MODEL_SCHEMA_MODULE,
        sanitized::READ_MODEL_SCHEMA_VERSION,
    ),
    (
        providers::CODEX_USAGE_SCHEMA_MODULE,
        providers::CODEX_USAGE_SCHEMA_VERSION,
    ),
    (
        providers::CLAUDE_USAGE_SCHEMA_MODULE,
        providers::CLAUDE_USAGE_SCHEMA_VERSION,
    ),
    (
        updater::DATABASE_SCHEMA_MODULE,
        updater::DATABASE_SCHEMA_VERSION,
    ),
    (COORDINATOR_SCHEMA_MODULE, COORDINATOR_SCHEMA_VERSION),
];

const TABLES: &[&str] = &[
    "claude_usage_daily",
    "claude_usage_files",
    "claude_usage_frames",
    "claude_usage_index_meta",
    "claude_usage_message_supersedes",
    "claude_usage_messages",
    "codex_account_usage_days",
    "codex_account_usage_meta",
    "codex_usage_file_days",
    "codex_usage_file_model_days",
    "codex_usage_files",
    "codex_usage_index_meta",
    "lifecycle_state",
    "provider_settings",
    "sanitized_desktop_state",
    "touchgrassbar_schema_versions",
    "touchgrassbar_update_state_v3",
];

const INDEXES: &[&str] = &[
    "claude_usage_frames_by_day",
    "claude_usage_messages_by_day",
    "claude_usage_messages_by_message",
    "claude_usage_messages_by_superseded_frame",
    "claude_usage_supersedes_by_superseded_frame",
    "codex_usage_model_days_by_day",
    "codex_usage_unpriced_model_days",
];

const VIEWS: &[&str] = &["touchgrassbar_update_state"];

const PRIMARY_KEYS: &[(&str, &[&str])] = &[
    ("claude_usage_daily", &["day"]),
    ("claude_usage_files", &["path"]),
    ("claude_usage_frames", &["frame_key"]),
    ("claude_usage_index_meta", &["key"]),
    (
        "claude_usage_message_supersedes",
        &["replacement_frame_key", "superseded_frame_key"],
    ),
    ("claude_usage_messages", &["frame_key"]),
    ("codex_account_usage_days", &["day"]),
    ("codex_account_usage_meta", &["singleton"]),
    ("codex_usage_file_days", &["path", "day"]),
    (
        "codex_usage_file_model_days",
        &["path", "day", "model", "pricing_input_tokens"],
    ),
    ("codex_usage_files", &["path"]),
    ("codex_usage_index_meta", &["key"]),
    ("lifecycle_state", &["singleton"]),
    ("provider_settings", &["provider"]),
    ("sanitized_desktop_state", &["singleton"]),
    ("touchgrassbar_schema_versions", &["module"]),
    ("touchgrassbar_update_state_v3", &["singleton"]),
];

const NULLABLE_COLUMNS: &[(&str, &[&str])] = &[
    (
        "claude_usage_daily",
        &["cost_usd", "pricing_basis", "pricing_fingerprint"],
    ),
    ("claude_usage_files", &["resume_anchor"]),
    (
        "claude_usage_messages",
        &[
            "supersedes_frame_key",
            "cache_creation_5m_input_tokens",
            "cache_creation_1h_input_tokens",
            "service_tier",
            "inference_geo",
            "speed",
            "web_search_requests",
            "web_fetch_requests",
            "code_execution_requests",
        ],
    ),
    (
        "codex_usage_file_days",
        &["priced_observed_through", "pricing_fingerprint"],
    ),
    (
        "codex_usage_file_model_days",
        &["cost_usd", "pricing_basis", "pricing_fingerprint"],
    ),
    (
        "codex_usage_files",
        &[
            "parsed_prefix_anchor",
            "deferred_until_day",
            "active_model",
            "baseline_is_inherited",
            "history_start_ordinal",
            "previous_input",
            "previous_cached_input",
            "previous_cache_write_input",
            "previous_output",
            "previous_reasoning_output",
            "previous_total",
        ],
    ),
    (
        "lifecycle_state",
        &["backfill_window_days", "display_name", "touch_grass_id"],
    ),
    (
        "touchgrassbar_update_state_v3",
        &[
            "last_automatic_check_at",
            "offered_version",
            "minimum_required_version",
        ],
    ),
];

const COLUMN_DEFAULTS: &[(&str, &str, &str)] = &[
    ("claude_usage_daily", "priced_tokens", "0"),
    ("codex_usage_files", "record_ordinal", "0"),
    ("codex_usage_files", "usage_excluded", "0"),
    ("lifecycle_state", "recovery_disclosure_pending", "0"),
    (
        "touchgrassbar_update_state_v3",
        "automatic_checks_enabled",
        "1",
    ),
];

const FOREIGN_KEYS: &[(&str, &str, &str, &str)] = &[
    (
        "claude_usage_message_supersedes",
        "replacement_frame_key",
        "claude_usage_frames",
        "frame_key",
    ),
    ("codex_usage_file_days", "path", "codex_usage_files", "path"),
    (
        "codex_usage_file_model_days",
        "path",
        "codex_usage_files",
        "path",
    ),
];

const INDEX_DEFINITIONS: &[(&str, &str, &[&str], Option<&str>)] = &[
    (
        "claude_usage_frames_by_day",
        "claude_usage_frames",
        &["day"],
        None,
    ),
    (
        "claude_usage_messages_by_day",
        "claude_usage_messages",
        &["day"],
        None,
    ),
    (
        "claude_usage_messages_by_message",
        "claude_usage_messages",
        &["message_key"],
        None,
    ),
    (
        "claude_usage_messages_by_superseded_frame",
        "claude_usage_messages",
        &["supersedes_frame_key"],
        None,
    ),
    (
        "claude_usage_supersedes_by_superseded_frame",
        "claude_usage_message_supersedes",
        &["superseded_frame_key", "parser_version"],
        None,
    ),
    (
        "codex_usage_model_days_by_day",
        "codex_usage_file_model_days",
        &["day"],
        None,
    ),
    (
        "codex_usage_unpriced_model_days",
        "codex_usage_file_model_days",
        &["day", "model", "cache_write_input_tokens"],
        Some("cost_usd is null"),
    ),
];

const TABLE_CHECKS: &[(&str, &[&str])] = &[
    (
        "claude_usage_daily",
        &[
            "check(coveragein('complete','partial'))",
            "check(revision>=1)",
        ],
    ),
    ("codex_account_usage_meta", &["check(singleton=1)"]),
    (
        "lifecycle_state",
        &[
            "check(singleton=1)",
            "check(bootstrap_completedin(0,1))",
            "check(profile_provisioningin('not-authorized','profile-pending','ready'))",
            "check(public_participation_authorizedin(0,1))",
            "check(profile_retry_pendingin(0,1))",
            "check(backfill_window_days=30)",
            "check(display_nameisnullor(length(trim(display_name))between1and40))",
            "check(recovery_disclosure_pendingin(0,1))",
        ],
    ),
    (
        "provider_settings",
        &[
            "check(providerin('codex','claude'))",
            "check(enabledin(0,1))",
        ],
    ),
    (
        "sanitized_desktop_state",
        &[
            "check(singleton=1)",
            "check(schema_version=5)",
            "check(contract_version=3)",
            "check(length(revision)>0andrevisionnotglob'*[^0-9]*')",
        ],
    ),
    ("touchgrassbar_schema_versions", &["check(version>=1)"]),
    (
        "touchgrassbar_update_state_v3",
        &[
            "check(singleton=1)",
            "check(automatic_checks_enabledin(0,1))",
            "check(offered_versionisnullorlength(offered_version)between1and64)",
            "check(minimum_required_versionisnullorlength(minimum_required_version)between1and64)",
        ],
    ),
];

const TABLE_COLUMNS: &[(&str, &[&str])] = &[
    (
        "claude_usage_daily",
        &[
            "day",
            "observed_tokens",
            "coverage",
            "observed_through",
            "revision",
            "priced_tokens",
            "cost_usd",
            "pricing_basis",
            "pricing_fingerprint",
        ],
    ),
    (
        "claude_usage_files",
        &[
            "path",
            "file_identity",
            "size_bytes",
            "modified_ns",
            "parsed_offset",
            "resume_anchor",
            "parser_version",
            "completion_state",
        ],
    ),
    (
        "claude_usage_frames",
        &["frame_key", "day", "observed_at", "parser_version"],
    ),
    ("claude_usage_index_meta", &["key", "value"]),
    (
        "claude_usage_message_supersedes",
        &[
            "replacement_frame_key",
            "superseded_frame_key",
            "parser_version",
        ],
    ),
    (
        "claude_usage_messages",
        &[
            "frame_key",
            "supersedes_frame_key",
            "message_key",
            "day",
            "observed_at",
            "model",
            "input_tokens",
            "cache_creation_input_tokens",
            "cache_read_input_tokens",
            "output_tokens",
            "cache_creation_5m_input_tokens",
            "cache_creation_1h_input_tokens",
            "service_tier",
            "inference_geo",
            "speed",
            "web_search_requests",
            "web_fetch_requests",
            "code_execution_requests",
            "has_unknown_paid_server_tool",
            "observed_tokens",
            "complete",
            "parser_version",
        ],
    ),
    ("codex_account_usage_days", &["day", "tokens"]),
    ("codex_account_usage_meta", &["singleton", "observed_at"]),
    (
        "codex_usage_file_days",
        &[
            "path",
            "day",
            "observed_tokens",
            "priced_tokens",
            "cost_usd",
            "complete",
            "observed_through",
            "priced_observed_through",
            "pricing_fingerprint",
        ],
    ),
    (
        "codex_usage_file_model_days",
        &[
            "path",
            "day",
            "model",
            "pricing_input_tokens",
            "input_tokens",
            "cached_input_tokens",
            "cache_write_input_tokens",
            "output_tokens",
            "reasoning_output_tokens",
            "observed_tokens",
            "cost_usd",
            "pricing_basis",
            "pricing_fingerprint",
            "complete",
            "observed_through",
        ],
    ),
    (
        "codex_usage_files",
        &[
            "path",
            "file_identity",
            "size_bytes",
            "modified_ns",
            "parsed_offset",
            "parsed_prefix_anchor",
            "parser_version",
            "completion_state",
            "deferred_until_day",
            "active_model",
            "baseline_is_inherited",
            "history_start_ordinal",
            "record_ordinal",
            "usage_excluded",
            "schema_supported",
            "previous_input",
            "previous_cached_input",
            "previous_cache_write_input",
            "previous_output",
            "previous_reasoning_output",
            "previous_total",
        ],
    ),
    ("codex_usage_index_meta", &["key", "value"]),
    (
        "lifecycle_state",
        &[
            "singleton",
            "bootstrap_completed",
            "profile_provisioning",
            "public_participation_authorized",
            "profile_retry_pending",
            "backfill_window_days",
            "display_name",
            "touch_grass_id",
            "recovery_disclosure_pending",
        ],
    ),
    ("provider_settings", &["provider", "enabled"]),
    (
        "sanitized_desktop_state",
        &[
            "singleton",
            "schema_version",
            "contract_version",
            "revision",
            "snapshot_json",
        ],
    ),
    ("touchgrassbar_schema_versions", &["module", "version"]),
    (
        "touchgrassbar_update_state",
        &[
            "singleton",
            "automatic_checks_enabled",
            "last_automatic_check_at",
            "offered_version",
            "minimum_required_version",
        ],
    ),
    (
        "touchgrassbar_update_state_v3",
        &[
            "singleton",
            "automatic_checks_enabled",
            "last_automatic_check_at",
            "offered_version",
            "minimum_required_version",
        ],
    ),
];

/// This value proves that every registered database Module is ready.
/// Online work must receive this value before it receives the database path.
#[derive(Clone, Debug)]
pub(crate) struct PreparedDatabase {
    path: PathBuf,
}

impl PreparedDatabase {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DatabaseOpenError {
    UnsupportedFuture { module: &'static str },
    MigrationFailed { stage: &'static str },
    InvariantFailed { invariant: &'static str },
}

impl DatabaseOpenError {
    pub(crate) fn diagnostic(&self) -> &'static str {
        match self {
            Self::UnsupportedFuture { .. } => "unsupported-future-database",
            Self::MigrationFailed { .. } => "known-migration-failed",
            Self::InvariantFailed { .. } => "database-invariant-failed",
        }
    }

    pub(crate) fn detail(&self) -> &'static str {
        match self {
            Self::UnsupportedFuture { module } => module,
            Self::MigrationFailed { stage } => stage,
            Self::InvariantFailed { invariant } => invariant,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PrepareFault {
    None,
    #[cfg(test)]
    BeforeBackupComplete,
    #[cfg(test)]
    AfterBackupComplete,
    #[cfg(test)]
    BeforeFinalCommit,
    #[cfg(test)]
    AfterCommitBeforeReady,
}

#[derive(Clone, Copy, Debug)]
struct SourceInspection {
    has_content: bool,
    needs_migration: bool,
}

pub(crate) fn prepare(path: &Path) -> Result<PreparedDatabase, DatabaseOpenError> {
    prepare_with_fault(path, PrepareFault::None)
}

fn prepare_with_fault(
    path: &Path,
    fault: PrepareFault,
) -> Result<PreparedDatabase, DatabaseOpenError> {
    let source = inspect_source(path)?;
    if !source.needs_migration {
        let connection = open_read_only(path, "open-ready")?;
        verify_invariants(&connection)?;
        drop(connection);
        validate_coordinator_recovery_state(path)?;
        cleanup_module_backups(path)?;
        finish_coordinator_migration(path)?;
        return Ok(PreparedDatabase {
            path: path.to_owned(),
        });
    }

    if source.has_content {
        create_coordinator_backup(path, fault)?;
        #[cfg(test)]
        if fault == PrepareFault::AfterBackupComplete {
            return Err(DatabaseOpenError::MigrationFailed {
                stage: "after-backup",
            });
        }
    }

    lifecycle::DesktopLifecycle::open(path).map_err(|_| DatabaseOpenError::MigrationFailed {
        stage: "desktop-lifecycle",
    })?;
    sanitized::prepare_database(path).map_err(|_| DatabaseOpenError::MigrationFailed {
        stage: "sanitized-desktop-state",
    })?;
    providers::prepare_usage_databases(path).map_err(|_| DatabaseOpenError::MigrationFailed {
        stage: "provider-usage-indexes",
    })?;
    updater::prepare_database(path).map_err(|_| DatabaseOpenError::MigrationFailed {
        stage: "update-state",
    })?;

    let mut connection =
        Connection::open(path).map_err(|_| DatabaseOpenError::MigrationFailed {
            stage: "open-final",
        })?;
    connection
        .busy_timeout(Duration::from_secs(2))
        .map_err(|_| DatabaseOpenError::MigrationFailed {
            stage: "configure-final",
        })?;
    connection
        .execute_batch("PRAGMA foreign_keys = ON;")
        .map_err(|_| DatabaseOpenError::MigrationFailed {
            stage: "configure-final",
        })?;
    let transaction = connection
        .transaction()
        .map_err(|_| DatabaseOpenError::MigrationFailed {
            stage: "begin-final",
        })?;
    transaction
        .execute(
            "INSERT INTO touchgrassbar_schema_versions(module, version) VALUES(?1, ?2)
             ON CONFLICT(module) DO UPDATE SET version = excluded.version",
            params![COORDINATOR_SCHEMA_MODULE, COORDINATOR_SCHEMA_VERSION],
        )
        .map_err(|_| DatabaseOpenError::MigrationFailed {
            stage: "write-version-vector",
        })?;
    transaction
        .pragma_update(None, "user_version", DATABASE_FORMAT_VERSION)
        .map_err(|_| DatabaseOpenError::MigrationFailed {
            stage: "write-database-format",
        })?;
    verify_invariants(&transaction)?;
    #[cfg(test)]
    if fault == PrepareFault::BeforeFinalCommit {
        return Err(DatabaseOpenError::MigrationFailed {
            stage: "before-final-commit",
        });
    }
    transaction
        .commit()
        .map_err(|_| DatabaseOpenError::MigrationFailed {
            stage: "commit-final",
        })?;
    #[cfg(test)]
    if fault == PrepareFault::AfterCommitBeforeReady {
        return Err(DatabaseOpenError::MigrationFailed {
            stage: "after-final-commit",
        });
    }
    verify_invariants(&connection)?;
    drop(connection);
    cleanup_module_backups(path)?;
    finish_coordinator_migration(path)?;
    Ok(PreparedDatabase {
        path: path.to_owned(),
    })
}

fn inspect_source(path: &Path) -> Result<SourceInspection, DatabaseOpenError> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Ok(SourceInspection {
                has_content: false,
                needs_migration: true,
            });
        }
        Err(_) => {
            return Err(DatabaseOpenError::MigrationFailed {
                stage: "inspect-source",
            });
        }
    };
    if metadata.len() == 0 {
        return Ok(SourceInspection {
            has_content: false,
            needs_migration: true,
        });
    }

    let connection = open_read_only(path, "inspect-source")?;
    reject_unknown_objects(&connection)?;
    let format_version = connection
        .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
        .map_err(|_| DatabaseOpenError::MigrationFailed {
            stage: "inspect-format",
        })?;
    if format_version > DATABASE_FORMAT_VERSION {
        return Err(DatabaseOpenError::UnsupportedFuture {
            module: "database-format",
        });
    }
    if format_version < 0 {
        return Err(DatabaseOpenError::MigrationFailed {
            stage: "inspect-format",
        });
    }

    let versions = read_version_rows(&connection)?;
    for (module, version) in &versions {
        let Some((known_module, current)) = MODULES
            .iter()
            .find(|(known_module, _)| *known_module == module)
        else {
            return Err(DatabaseOpenError::UnsupportedFuture {
                module: "unregistered-module",
            });
        };
        if *version > *current {
            return Err(DatabaseOpenError::UnsupportedFuture {
                module: known_module,
            });
        }
        if *version < 1 {
            return Err(DatabaseOpenError::MigrationFailed {
                stage: "inspect-version-vector",
            });
        }
    }
    inspect_registered_modules(&connection)?;

    let coordinator_version = versions
        .iter()
        .find(|(module, _)| module == COORDINATOR_SCHEMA_MODULE)
        .map(|(_, version)| *version);
    if coordinator_version == Some(COORDINATOR_SCHEMA_VERSION) {
        let complete = format_version == DATABASE_FORMAT_VERSION
            && MODULES.iter().all(|(module, current)| {
                versions
                    .iter()
                    .any(|(stored_module, stored)| stored_module == module && stored == current)
            })
            && versions.len() == MODULES.len();
        if !complete {
            return Err(DatabaseOpenError::InvariantFailed {
                invariant: "ready-version-vector",
            });
        }
        return Ok(SourceInspection {
            has_content: true,
            needs_migration: false,
        });
    }

    Ok(SourceInspection {
        has_content: true,
        needs_migration: true,
    })
}

fn inspect_registered_modules(connection: &Connection) -> Result<(), DatabaseOpenError> {
    let explicit_lifecycle_version =
        lifecycle::lifecycle_schema_version(connection).map_err(|_| {
            DatabaseOpenError::MigrationFailed {
                stage: "inspect-desktop-lifecycle",
            }
        })?;
    let database_format = connection
        .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
        .map_err(|_| DatabaseOpenError::MigrationFailed {
            stage: "inspect-desktop-lifecycle",
        })?;
    let lifecycle_version = explicit_lifecycle_version.unwrap_or(database_format);
    let has_lifecycle = table_exists(connection, "lifecycle_state")?;
    let has_provider_settings = table_exists(connection, "provider_settings")?;
    if !matches!(lifecycle_version, 0 | 4 | 5)
        || (lifecycle_version == 0 && (has_lifecycle || has_provider_settings))
        || (lifecycle_version >= 4 && !has_lifecycle)
        || (lifecycle_version == 4 && has_provider_settings)
        || (lifecycle_version == 5 && !has_provider_settings)
    {
        return Err(DatabaseOpenError::MigrationFailed {
            stage: "inspect-desktop-lifecycle",
        });
    }

    let read_model_version = sanitized::read_model_schema_version(connection).map_err(|_| {
        DatabaseOpenError::MigrationFailed {
            stage: "inspect-sanitized-state",
        }
    })?;
    let has_read_model = table_exists(connection, "sanitized_desktop_state")?;
    if !matches!(read_model_version, 0 | 4 | 5)
        || (read_model_version == 0 && has_read_model)
        || (read_model_version > 0 && !has_read_model)
    {
        return Err(DatabaseOpenError::MigrationFailed {
            stage: "inspect-sanitized-state",
        });
    }

    let codex_version = providers::codex_usage_schema_version(connection).map_err(|_| {
        DatabaseOpenError::MigrationFailed {
            stage: "inspect-codex-usage",
        }
    })?;
    let codex_tables = [
        "codex_usage_index_meta",
        "codex_account_usage_meta",
        "codex_account_usage_days",
        "codex_usage_files",
        "codex_usage_file_model_days",
        "codex_usage_file_days",
    ];
    let codex_table_count = count_tables(connection, &codex_tables)?;
    if !matches!(codex_version, 0 | 2 | 3)
        || (codex_version == 0 && codex_table_count != 0)
        || (codex_version >= 2 && codex_table_count != codex_tables.len())
    {
        return Err(DatabaseOpenError::MigrationFailed {
            stage: "inspect-codex-usage",
        });
    }

    let claude_version = providers::claude_usage_schema_version(connection).map_err(|_| {
        DatabaseOpenError::MigrationFailed {
            stage: "inspect-claude-usage",
        }
    })?;
    let claude_tables = [
        "claude_usage_index_meta",
        "claude_usage_files",
        "claude_usage_messages",
        "claude_usage_frames",
        "claude_usage_message_supersedes",
        "claude_usage_daily",
    ];
    let claude_table_count = count_tables(connection, &claude_tables)?;
    if !matches!(claude_version, 0 | 3 | 4)
        || (claude_version == 0 && claude_table_count != 0)
        || (claude_version >= 3 && claude_table_count != claude_tables.len())
    {
        return Err(DatabaseOpenError::MigrationFailed {
            stage: "inspect-claude-usage",
        });
    }

    let (update_version, update_explicit) =
        updater::update_schema_version(connection).map_err(|_| {
            DatabaseOpenError::MigrationFailed {
                stage: "inspect-update-state",
            }
        })?;
    let has_update_storage = table_exists(connection, "touchgrassbar_update_state_v3")?;
    let has_update_table = table_exists(connection, "touchgrassbar_update_state")?;
    let has_update_view = object_exists(connection, "view", "touchgrassbar_update_state")?;
    if !matches!(update_version, 0..=3)
        || (update_version < 3 && (has_update_storage || has_update_view))
        || (update_version > 0 && update_version < 3 && !has_update_table)
        || (update_version == 3
            && (!update_explicit
                || !has_update_storage
                || has_update_table
                || !has_update_view
                || !object_has_columns(
                    connection,
                    "touchgrassbar_update_state_v3",
                    &[
                        "singleton",
                        "automatic_checks_enabled",
                        "last_automatic_check_at",
                        "offered_version",
                        "minimum_required_version",
                    ],
                )?
                || !object_has_columns(
                    connection,
                    "touchgrassbar_update_state",
                    &[
                        "singleton",
                        "automatic_checks_enabled",
                        "last_automatic_check_at",
                        "offered_version",
                        "minimum_required_version",
                    ],
                )?))
    {
        return Err(DatabaseOpenError::MigrationFailed {
            stage: "inspect-update-state",
        });
    }
    inspect_known_table_columns(connection)?;
    Ok(())
}

fn inspect_known_table_columns(connection: &Connection) -> Result<(), DatabaseOpenError> {
    for (table, expected) in TABLE_COLUMNS {
        if matches!(
            *table,
            "touchgrassbar_update_state" | "touchgrassbar_update_state_v3"
        ) {
            continue;
        }
        if table_exists(connection, table)? && !object_has_columns(connection, table, expected)? {
            return Err(DatabaseOpenError::MigrationFailed {
                stage: "inspect-table-columns",
            });
        }
    }
    Ok(())
}

fn object_has_columns(
    connection: &Connection,
    object: &str,
    expected: &[&str],
) -> Result<bool, DatabaseOpenError> {
    let mut statement = connection
        .prepare("SELECT name FROM pragma_table_info(?1) ORDER BY cid")
        .map_err(|_| DatabaseOpenError::MigrationFailed {
            stage: "inspect-table-columns",
        })?;
    let mut columns = statement
        .query_map([object], |row| row.get::<_, String>(0))
        .map_err(|_| DatabaseOpenError::MigrationFailed {
            stage: "inspect-table-columns",
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| DatabaseOpenError::MigrationFailed {
            stage: "inspect-table-columns",
        })?;
    let mut expected = expected.to_vec();
    columns.sort();
    expected.sort();
    Ok(columns
        .iter()
        .map(String::as_str)
        .eq(expected.iter().copied()))
}

fn count_tables(connection: &Connection, tables: &[&str]) -> Result<usize, DatabaseOpenError> {
    let mut count = 0;
    for table in tables {
        count += usize::from(table_exists(connection, table)?);
    }
    Ok(count)
}

fn open_read_only(path: &Path, stage: &'static str) -> Result<Connection, DatabaseOpenError> {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|_| DatabaseOpenError::MigrationFailed { stage })
}

fn read_version_rows(connection: &Connection) -> Result<Vec<(String, i64)>, DatabaseOpenError> {
    if !table_exists(connection, "touchgrassbar_schema_versions")? {
        return Ok(Vec::new());
    }
    let mut statement = connection
        .prepare("SELECT module, version FROM touchgrassbar_schema_versions ORDER BY module")
        .map_err(|_| DatabaseOpenError::MigrationFailed {
            stage: "inspect-version-vector",
        })?;
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(|_| DatabaseOpenError::MigrationFailed {
            stage: "inspect-version-vector",
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| DatabaseOpenError::MigrationFailed {
            stage: "inspect-version-vector",
        })
}

fn table_exists(connection: &Connection, table: &str) -> Result<bool, DatabaseOpenError> {
    object_exists(connection, "table", table)
}

fn object_exists(
    connection: &Connection,
    object_type: &str,
    name: &str,
) -> Result<bool, DatabaseOpenError> {
    connection
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM sqlite_schema WHERE type = ?1 AND name = ?2
             )",
            params![object_type, name],
            |row| row.get(0),
        )
        .map_err(|_| DatabaseOpenError::MigrationFailed {
            stage: "inspect-objects",
        })
}

fn reject_unknown_objects(connection: &Connection) -> Result<(), DatabaseOpenError> {
    let mut statement = connection
        .prepare(
            "SELECT type, name FROM sqlite_schema
             WHERE type IN ('table', 'index', 'trigger', 'view')
               AND name NOT LIKE 'sqlite_%'
             ORDER BY type, name",
        )
        .map_err(|_| DatabaseOpenError::MigrationFailed {
            stage: "inspect-objects",
        })?;
    let objects = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|_| DatabaseOpenError::MigrationFailed {
            stage: "inspect-objects",
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| DatabaseOpenError::MigrationFailed {
            stage: "inspect-objects",
        })?;
    if objects
        .iter()
        .any(|(object_type, name)| match object_type.as_str() {
            "table" => !TABLES.contains(&name.as_str()) && !VIEWS.contains(&name.as_str()),
            "index" => !INDEXES.contains(&name.as_str()),
            "view" => !VIEWS.contains(&name.as_str()),
            _ => true,
        })
    {
        return Err(DatabaseOpenError::UnsupportedFuture {
            module: "unregistered-object",
        });
    }
    Ok(())
}

fn coordinator_backup_path(path: &Path) -> PathBuf {
    path.with_extension("sqlite3.compatibility.backup")
}

fn coordinator_backup_partial_path(path: &Path) -> PathBuf {
    path.with_extension("sqlite3.compatibility.backup.partial")
}

fn coordinator_marker_path(path: &Path) -> PathBuf {
    path.with_extension("sqlite3.compatibility.in-progress")
}

fn coordinator_marker_partial_path(path: &Path) -> PathBuf {
    path.with_extension("sqlite3.compatibility.in-progress.partial")
}

fn create_coordinator_backup(path: &Path, _fault: PrepareFault) -> Result<(), DatabaseOpenError> {
    let backup_path = coordinator_backup_path(path);
    if backup_path.exists() && coordinator_marker_path(path).exists() {
        return validate_coordinator_backup(&backup_path);
    }
    let partial_path = coordinator_backup_partial_path(path);
    if partial_path.exists() {
        fs::remove_file(&partial_path).map_err(|_| DatabaseOpenError::MigrationFailed {
            stage: "replace-partial-backup",
        })?;
    }
    let source = open_read_only(path, "open-backup-source")?;
    source
        .backup(rusqlite::MAIN_DB, &partial_path, None)
        .map_err(|_| DatabaseOpenError::MigrationFailed {
            stage: "copy-backup",
        })?;
    #[cfg(test)]
    if _fault == PrepareFault::BeforeBackupComplete {
        return Err(DatabaseOpenError::MigrationFailed {
            stage: "before-backup-complete",
        });
    }
    fs::File::open(&partial_path)
        .and_then(|file| file.sync_all())
        .map_err(|_| DatabaseOpenError::MigrationFailed {
            stage: "sync-backup",
        })?;
    validate_coordinator_backup(&partial_path)?;
    fs::rename(&partial_path, &backup_path).map_err(|_| DatabaseOpenError::MigrationFailed {
        stage: "publish-backup",
    })?;
    let parent = backup_path
        .parent()
        .ok_or(DatabaseOpenError::MigrationFailed {
            stage: "sync-backup-directory",
        })?;
    fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| DatabaseOpenError::MigrationFailed {
            stage: "sync-backup-directory",
        })?;
    publish_coordinator_marker(path)
}

fn publish_coordinator_marker(path: &Path) -> Result<(), DatabaseOpenError> {
    let marker_path = coordinator_marker_path(path);
    let partial_path = coordinator_marker_partial_path(path);
    if partial_path.exists() {
        fs::remove_file(&partial_path).map_err(|_| DatabaseOpenError::MigrationFailed {
            stage: "replace-partial-marker",
        })?;
    }
    let mut marker =
        fs::File::create(&partial_path).map_err(|_| DatabaseOpenError::MigrationFailed {
            stage: "write-migration-marker",
        })?;
    marker
        .write_all(b"migration-in-progress\n")
        .and_then(|()| marker.sync_all())
        .map_err(|_| DatabaseOpenError::MigrationFailed {
            stage: "write-migration-marker",
        })?;
    fs::rename(&partial_path, &marker_path).map_err(|_| DatabaseOpenError::MigrationFailed {
        stage: "publish-migration-marker",
    })?;
    sync_parent_directory(&marker_path, "sync-marker-directory")
}

fn validate_coordinator_recovery_state(path: &Path) -> Result<(), DatabaseOpenError> {
    let backup_path = coordinator_backup_path(path);
    if coordinator_marker_path(path).exists() && !backup_path.exists() {
        return Err(DatabaseOpenError::MigrationFailed {
            stage: "validate-backup",
        });
    }
    if backup_path.exists() {
        validate_coordinator_backup(&backup_path)?;
    }
    Ok(())
}

fn finish_coordinator_migration(path: &Path) -> Result<(), DatabaseOpenError> {
    for marker in [
        coordinator_marker_path(path),
        coordinator_marker_partial_path(path),
    ] {
        match fs::remove_file(marker) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(_) => {
                return Err(DatabaseOpenError::MigrationFailed {
                    stage: "finish-migration",
                });
            }
        }
    }
    sync_parent_directory(path, "sync-migration-directory")
}

fn sync_parent_directory(path: &Path, stage: &'static str) -> Result<(), DatabaseOpenError> {
    let parent = path
        .parent()
        .ok_or(DatabaseOpenError::MigrationFailed { stage })?;
    fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| DatabaseOpenError::MigrationFailed { stage })
}

fn validate_coordinator_backup(path: &Path) -> Result<(), DatabaseOpenError> {
    let connection = open_read_only(path, "open-backup")?;
    let integrity = connection
        .query_row("PRAGMA quick_check", [], |row| row.get::<_, String>(0))
        .map_err(|_| DatabaseOpenError::MigrationFailed {
            stage: "validate-backup",
        })?;
    if integrity != "ok" {
        return Err(DatabaseOpenError::MigrationFailed {
            stage: "validate-backup",
        });
    }
    let format_version = connection
        .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
        .map_err(|_| DatabaseOpenError::MigrationFailed {
            stage: "validate-backup-source",
        })?;
    let versions =
        read_version_rows(&connection).map_err(|_| DatabaseOpenError::MigrationFailed {
            stage: "validate-backup-source",
        })?;
    let known_source = (0..DATABASE_FORMAT_VERSION).contains(&format_version)
        && versions.iter().all(|(module, version)| {
            module != COORDINATOR_SCHEMA_MODULE
                && MODULES
                    .iter()
                    .any(|(known, current)| module == known && *version >= 1 && version <= current)
        });
    if !known_source {
        return Err(DatabaseOpenError::MigrationFailed {
            stage: "validate-backup-source",
        });
    }
    reject_unknown_objects(&connection).map_err(|_| DatabaseOpenError::MigrationFailed {
        stage: "validate-backup-source",
    })?;
    inspect_registered_modules(&connection).map_err(|_| DatabaseOpenError::MigrationFailed {
        stage: "validate-backup-source",
    })?;
    Ok(())
}

fn verify_invariants(connection: &Connection) -> Result<(), DatabaseOpenError> {
    let integrity = connection
        .query_row("PRAGMA quick_check", [], |row| row.get::<_, String>(0))
        .map_err(|_| DatabaseOpenError::InvariantFailed {
            invariant: "integrity",
        })?;
    if integrity != "ok" {
        return Err(DatabaseOpenError::InvariantFailed {
            invariant: "integrity",
        });
    }
    let mut foreign_keys = connection
        .prepare("PRAGMA foreign_key_check")
        .map_err(|_| DatabaseOpenError::InvariantFailed {
            invariant: "foreign-keys",
        })?;
    if foreign_keys
        .query([])
        .and_then(|mut rows| rows.next().map(|row| row.is_some()))
        .map_err(|_| DatabaseOpenError::InvariantFailed {
            invariant: "foreign-keys",
        })?
    {
        return Err(DatabaseOpenError::InvariantFailed {
            invariant: "foreign-keys",
        });
    }

    let format_version = connection
        .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
        .map_err(|_| DatabaseOpenError::InvariantFailed {
            invariant: "database-format",
        })?;
    if format_version != DATABASE_FORMAT_VERSION {
        return Err(DatabaseOpenError::InvariantFailed {
            invariant: "database-format",
        });
    }

    let mut expected_versions = MODULES
        .iter()
        .map(|(module, version)| ((*module).to_owned(), *version))
        .collect::<Vec<_>>();
    expected_versions.sort();
    if read_version_rows(connection)? != expected_versions {
        return Err(DatabaseOpenError::InvariantFailed {
            invariant: "version-vector",
        });
    }
    reject_unknown_objects(connection).map_err(|_| DatabaseOpenError::InvariantFailed {
        invariant: "object-registry",
    })?;
    let mut stored_tables = object_names(connection, "table")?;
    stored_tables.sort();
    if stored_tables != TABLES {
        return Err(DatabaseOpenError::InvariantFailed {
            invariant: "table-registry",
        });
    }
    let mut stored_indexes = object_names(connection, "index")?;
    stored_indexes.sort();
    if stored_indexes != INDEXES {
        return Err(DatabaseOpenError::InvariantFailed {
            invariant: "index-registry",
        });
    }
    let mut stored_views = object_names(connection, "view")?;
    stored_views.sort();
    if stored_views != VIEWS {
        return Err(DatabaseOpenError::InvariantFailed {
            invariant: "view-registry",
        });
    }
    verify_table_columns(connection)?;
    verify_table_definitions(connection)?;
    verify_foreign_key_definitions(connection)?;
    verify_index_definitions(connection)?;
    verify_view_definitions(connection)?;

    let lifecycle_profile = verify_lifecycle(connection)?;
    let sanitized_state = sanitized::read_database_state(connection).map_err(|_| {
        DatabaseOpenError::InvariantFailed {
            invariant: "sanitized-state",
        }
    })?;
    if sanitized_state.profile != lifecycle_profile {
        return Err(DatabaseOpenError::InvariantFailed {
            invariant: "profile-projection",
        });
    }
    verify_update_state(connection)?;
    verify_usage_indexes(connection)?;
    Ok(())
}

fn object_names(
    connection: &Connection,
    object_type: &str,
) -> Result<Vec<&'static str>, DatabaseOpenError> {
    let mut statement = connection
        .prepare(
            "SELECT name FROM sqlite_schema
             WHERE type = ?1 AND name NOT LIKE 'sqlite_%'
             ORDER BY name",
        )
        .map_err(|_| DatabaseOpenError::InvariantFailed {
            invariant: "object-registry",
        })?;
    let names = statement
        .query_map([object_type], |row| row.get::<_, String>(0))
        .map_err(|_| DatabaseOpenError::InvariantFailed {
            invariant: "object-registry",
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| DatabaseOpenError::InvariantFailed {
            invariant: "object-registry",
        })?;
    names
        .into_iter()
        .map(|name| {
            TABLES
                .iter()
                .chain(INDEXES.iter())
                .chain(VIEWS.iter())
                .copied()
                .find(|known| *known == name)
                .ok_or(DatabaseOpenError::InvariantFailed {
                    invariant: "object-registry",
                })
        })
        .collect()
}

fn verify_table_columns(connection: &Connection) -> Result<(), DatabaseOpenError> {
    for (table, expected) in TABLE_COLUMNS {
        let mut statement = connection
            .prepare("SELECT name FROM pragma_table_info(?1) ORDER BY cid")
            .map_err(|_| DatabaseOpenError::InvariantFailed {
                invariant: "table-columns",
            })?;
        let mut columns = statement
            .query_map([table], |row| row.get::<_, String>(0))
            .map_err(|_| DatabaseOpenError::InvariantFailed {
                invariant: "table-columns",
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| DatabaseOpenError::InvariantFailed {
                invariant: "table-columns",
            })?;
        let mut expected = expected
            .iter()
            .map(|column| (*column).to_owned())
            .collect::<Vec<_>>();
        columns.sort();
        expected.sort();
        if columns != expected {
            return Err(DatabaseOpenError::InvariantFailed {
                invariant: "table-columns",
            });
        }
    }
    Ok(())
}

fn verify_table_definitions(connection: &Connection) -> Result<(), DatabaseOpenError> {
    for table in TABLES {
        let mut statement = connection
            .prepare(
                "SELECT name, \"notnull\", dflt_value, pk
                 FROM pragma_table_info(?1) ORDER BY cid",
            )
            .map_err(|_| DatabaseOpenError::InvariantFailed {
                invariant: "table-definitions",
            })?;
        let columns = statement
            .query_map([table], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, bool>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })
            .map_err(|_| DatabaseOpenError::InvariantFailed {
                invariant: "table-definitions",
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| DatabaseOpenError::InvariantFailed {
                invariant: "table-definitions",
            })?;
        let mut primary_key = columns
            .iter()
            .filter(|(_, _, _, position)| *position > 0)
            .map(|(name, _, _, position)| (*position, name.as_str()))
            .collect::<Vec<_>>();
        primary_key.sort_by_key(|(position, _)| *position);
        let expected_primary_key = PRIMARY_KEYS
            .iter()
            .find(|(known_table, _)| known_table == table)
            .map(|(_, columns)| *columns)
            .ok_or(DatabaseOpenError::InvariantFailed {
                invariant: "table-definitions",
            })?;
        if !primary_key
            .iter()
            .map(|(_, name)| *name)
            .eq(expected_primary_key.iter().copied())
        {
            return Err(DatabaseOpenError::InvariantFailed {
                invariant: "table-definitions",
            });
        }

        let nullable = NULLABLE_COLUMNS
            .iter()
            .find(|(known_table, _)| known_table == table)
            .map(|(_, columns)| *columns)
            .unwrap_or_default();
        for (name, not_null, default, primary_key_position) in &columns {
            let is_nullable = !not_null && *primary_key_position == 0;
            if is_nullable != nullable.contains(&name.as_str()) {
                return Err(DatabaseOpenError::InvariantFailed {
                    invariant: "table-definitions",
                });
            }
            let expected_default = COLUMN_DEFAULTS
                .iter()
                .find(|(known_table, column, _)| known_table == table && column == name)
                .map(|(_, _, value)| normalize_sql(value));
            if default.as_deref().map(normalize_sql) != expected_default {
                return Err(DatabaseOpenError::InvariantFailed {
                    invariant: "table-definitions",
                });
            }
        }

        let schema = schema_sql(connection, "table", table, "table-definitions")?;
        let expected_checks = TABLE_CHECKS
            .iter()
            .find(|(known_table, _)| known_table == table)
            .map(|(_, checks)| *checks)
            .unwrap_or_default();
        if schema.matches("check(").count() != expected_checks.len()
            || expected_checks.iter().any(|check| !schema.contains(check))
        {
            return Err(DatabaseOpenError::InvariantFailed {
                invariant: "table-definitions",
            });
        }
    }
    Ok(())
}

fn verify_foreign_key_definitions(connection: &Connection) -> Result<(), DatabaseOpenError> {
    for table in TABLES {
        let mut statement = connection
            .prepare(
                "SELECT \"from\", \"table\", \"to\", on_update, on_delete, match
                 FROM pragma_foreign_key_list(?1)",
            )
            .map_err(|_| DatabaseOpenError::InvariantFailed {
                invariant: "foreign-key-definitions",
            })?;
        let mut stored = statement
            .query_map([table], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            })
            .map_err(|_| DatabaseOpenError::InvariantFailed {
                invariant: "foreign-key-definitions",
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| DatabaseOpenError::InvariantFailed {
                invariant: "foreign-key-definitions",
            })?;
        let mut expected = FOREIGN_KEYS
            .iter()
            .filter(|(known_table, _, _, _)| known_table == table)
            .map(|(_, from, target_table, target_column)| {
                (
                    (*from).to_owned(),
                    (*target_table).to_owned(),
                    (*target_column).to_owned(),
                    "NO ACTION".to_owned(),
                    "CASCADE".to_owned(),
                    "NONE".to_owned(),
                )
            })
            .collect::<Vec<_>>();
        stored.sort();
        expected.sort();
        if stored != expected {
            return Err(DatabaseOpenError::InvariantFailed {
                invariant: "foreign-key-definitions",
            });
        }
    }
    Ok(())
}

fn verify_index_definitions(connection: &Connection) -> Result<(), DatabaseOpenError> {
    for table in TABLES {
        let has_unique_constraint = connection
            .query_row(
                "SELECT EXISTS(
                   SELECT 1 FROM pragma_index_list(?1) WHERE origin = 'u'
                 )",
                [table],
                |row| row.get::<_, bool>(0),
            )
            .map_err(|_| DatabaseOpenError::InvariantFailed {
                invariant: "index-definitions",
            })?;
        if has_unique_constraint {
            return Err(DatabaseOpenError::InvariantFailed {
                invariant: "index-definitions",
            });
        }
    }

    for (name, table, expected_columns, predicate) in INDEX_DEFINITIONS {
        let (unique, origin, partial) = connection
            .query_row(
                "SELECT \"unique\", origin, partial
                 FROM pragma_index_list(?1) WHERE name = ?2",
                params![table, name],
                |row| {
                    Ok((
                        row.get::<_, bool>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, bool>(2)?,
                    ))
                },
            )
            .map_err(|_| DatabaseOpenError::InvariantFailed {
                invariant: "index-definitions",
            })?;
        if unique || origin != "c" || partial != predicate.is_some() {
            return Err(DatabaseOpenError::InvariantFailed {
                invariant: "index-definitions",
            });
        }
        let mut statement = connection
            .prepare("SELECT name FROM pragma_index_info(?1) ORDER BY seqno")
            .map_err(|_| DatabaseOpenError::InvariantFailed {
                invariant: "index-definitions",
            })?;
        let columns = statement
            .query_map([name], |row| row.get::<_, String>(0))
            .map_err(|_| DatabaseOpenError::InvariantFailed {
                invariant: "index-definitions",
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| DatabaseOpenError::InvariantFailed {
                invariant: "index-definitions",
            })?;
        if !columns
            .iter()
            .map(String::as_str)
            .eq(expected_columns.iter().copied())
        {
            return Err(DatabaseOpenError::InvariantFailed {
                invariant: "index-definitions",
            });
        }

        let mut expected_sql = format!(
            "create index {name} on {table}({})",
            expected_columns.join(",")
        );
        if let Some(predicate) = predicate {
            expected_sql.push_str(" where ");
            expected_sql.push_str(predicate);
        }
        if schema_sql(connection, "index", name, "index-definitions")?
            != normalize_sql(&expected_sql)
        {
            return Err(DatabaseOpenError::InvariantFailed {
                invariant: "index-definitions",
            });
        }
    }
    Ok(())
}

fn verify_view_definitions(connection: &Connection) -> Result<(), DatabaseOpenError> {
    let expected = normalize_sql(
        "CREATE VIEW touchgrassbar_update_state AS
         SELECT singleton, automatic_checks_enabled, last_automatic_check_at,
                offered_version, minimum_required_version
         FROM touchgrassbar_update_state_v3",
    );
    if schema_sql(
        connection,
        "view",
        "touchgrassbar_update_state",
        "view-definitions",
    )? != expected
    {
        return Err(DatabaseOpenError::InvariantFailed {
            invariant: "view-definitions",
        });
    }
    Ok(())
}

fn schema_sql(
    connection: &Connection,
    object_type: &str,
    name: &str,
    invariant: &'static str,
) -> Result<String, DatabaseOpenError> {
    connection
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE type = ?1 AND name = ?2",
            params![object_type, name],
            |row| row.get::<_, String>(0),
        )
        .map(|sql| normalize_sql(&sql))
        .map_err(|_| DatabaseOpenError::InvariantFailed { invariant })
}

fn normalize_sql(value: &str) -> String {
    value
        .chars()
        .filter(|character| !character.is_ascii_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

fn verify_lifecycle(
    connection: &Connection,
) -> Result<sanitized::SanitizedProfileOutcome, DatabaseOpenError> {
    let invalid_lifecycle = connection
        .query_row(
            "SELECT COUNT(*) != 1 OR EXISTS(
               SELECT 1 FROM lifecycle_state
               WHERE singleton != 1
                  OR bootstrap_completed NOT IN (0, 1)
                  OR profile_provisioning NOT IN ('not-authorized', 'profile-pending', 'ready')
                  OR public_participation_authorized NOT IN (0, 1)
                  OR profile_retry_pending NOT IN (0, 1)
                  OR recovery_disclosure_pending NOT IN (0, 1)
                  OR (backfill_window_days IS NOT NULL AND backfill_window_days != 30)
                  OR (display_name IS NOT NULL AND length(trim(display_name)) NOT BETWEEN 1 AND 40)
                  OR NOT (
                    (
                      bootstrap_completed = 0
                      AND profile_provisioning = 'not-authorized'
                      AND public_participation_authorized = 0
                      AND profile_retry_pending = 0
                      AND backfill_window_days IS NULL
                      AND display_name IS NULL
                      AND touch_grass_id IS NULL
                      AND recovery_disclosure_pending = 0
                    ) OR (
                      bootstrap_completed = 1
                      AND profile_provisioning = 'profile-pending'
                      AND public_participation_authorized = 1
                      AND profile_retry_pending = 1
                      AND backfill_window_days = 30
                      AND display_name IS NOT NULL
                      AND touch_grass_id IS NULL
                      AND recovery_disclosure_pending = 0
                    ) OR (
                      bootstrap_completed = 1
                      AND profile_provisioning = 'ready'
                      AND public_participation_authorized = 1
                      AND profile_retry_pending = 0
                      AND backfill_window_days = 30
                      AND display_name IS NOT NULL
                      AND touch_grass_id IS NOT NULL
                    )
                  )
             ) FROM lifecycle_state",
            [],
            |row| row.get::<_, bool>(0),
        )
        .map_err(|_| DatabaseOpenError::InvariantFailed {
            invariant: "lifecycle-state",
        })?;
    if invalid_lifecycle {
        return Err(DatabaseOpenError::InvariantFailed {
            invariant: "lifecycle-state",
        });
    }
    let valid_settings = connection
        .query_row(
            "SELECT COUNT(*) = 2
               AND SUM(provider = 'codex') = 1
               AND SUM(provider = 'claude') = 1
               AND SUM(enabled NOT IN (0, 1)) = 0
             FROM provider_settings",
            [],
            |row| row.get::<_, bool>(0),
        )
        .map_err(|_| DatabaseOpenError::InvariantFailed {
            invariant: "provider-settings",
        })?;
    if !valid_settings {
        return Err(DatabaseOpenError::InvariantFailed {
            invariant: "provider-settings",
        });
    }
    let (status, display_name, touch_grass_id) = connection
        .query_row(
            "SELECT profile_provisioning, display_name, touch_grass_id
             FROM lifecycle_state WHERE singleton = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .map_err(|_| DatabaseOpenError::InvariantFailed {
            invariant: "lifecycle-state",
        })?;
    match (status.as_str(), display_name, touch_grass_id) {
        ("not-authorized", None, None) => Ok(sanitized::SanitizedProfileOutcome::NotAuthorized),
        ("profile-pending", Some(_), None) => {
            Ok(sanitized::SanitizedProfileOutcome::ProfilePending)
        }
        ("ready", Some(display_name), Some(touch_grass_id)) => {
            Ok(sanitized::SanitizedProfileOutcome::Ready {
                display_name,
                touch_grass_id,
            })
        }
        _ => Err(DatabaseOpenError::InvariantFailed {
            invariant: "lifecycle-state",
        }),
    }
}

fn verify_update_state(connection: &Connection) -> Result<(), DatabaseOpenError> {
    let valid = connection
        .query_row(
            "SELECT COUNT(*) = 1 AND SUM(
               singleton = 1
               AND automatic_checks_enabled IN (0, 1)
               AND (offered_version IS NULL OR length(offered_version) BETWEEN 1 AND 64)
               AND (
                 minimum_required_version IS NULL OR
                 length(minimum_required_version) BETWEEN 1 AND 64
               )
             ) = 1
             FROM touchgrassbar_update_state_v3",
            [],
            |row| row.get::<_, bool>(0),
        )
        .map_err(|_| DatabaseOpenError::InvariantFailed {
            invariant: "update-state",
        })?;
    if !valid {
        return Err(DatabaseOpenError::InvariantFailed {
            invariant: "update-state",
        });
    }
    let (last_check, offered, minimum) = connection
        .query_row(
            "SELECT last_automatic_check_at, offered_version, minimum_required_version
             FROM touchgrassbar_update_state_v3 WHERE singleton = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, Option<i64>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .map_err(|_| DatabaseOpenError::InvariantFailed {
            invariant: "update-state",
        })?;
    if last_check.is_some_and(|timestamp| timestamp < 0) {
        return Err(DatabaseOpenError::InvariantFailed {
            invariant: "update-state",
        });
    }
    let offered = offered
        .as_deref()
        .map(semver::Version::parse)
        .transpose()
        .map_err(|_| DatabaseOpenError::InvariantFailed {
            invariant: "update-state",
        })?;
    let minimum = minimum
        .as_deref()
        .map(semver::Version::parse)
        .transpose()
        .map_err(|_| DatabaseOpenError::InvariantFailed {
            invariant: "update-state",
        })?;
    if offered
        .as_ref()
        .zip(minimum.as_ref())
        .is_some_and(|(offered, minimum)| offered < minimum)
    {
        return Err(DatabaseOpenError::InvariantFailed {
            invariant: "update-state",
        });
    }
    Ok(())
}

fn verify_usage_indexes(connection: &Connection) -> Result<(), DatabaseOpenError> {
    if providers::codex_usage_schema_version(connection).map_err(|_| {
        DatabaseOpenError::InvariantFailed {
            invariant: "codex-usage-index",
        }
    })? != providers::CODEX_USAGE_SCHEMA_VERSION
    {
        return Err(DatabaseOpenError::InvariantFailed {
            invariant: "codex-usage-index",
        });
    }
    if providers::claude_usage_schema_version(connection).map_err(|_| {
        DatabaseOpenError::InvariantFailed {
            invariant: "claude-usage-index",
        }
    })? != providers::CLAUDE_USAGE_SCHEMA_VERSION
    {
        return Err(DatabaseOpenError::InvariantFailed {
            invariant: "claude-usage-index",
        });
    }
    for (invariant, query) in [
        (
            "codex-usage-values",
            "SELECT EXISTS(
               SELECT 1 FROM codex_account_usage_days
               WHERE strftime('%Y-%m-%d', day, '+0 days') IS NULL
                  OR strftime('%Y-%m-%d', day, '+0 days') != day
                  OR length(day) != 10 OR tokens < 0
             ) OR EXISTS(
               SELECT 1 FROM codex_usage_file_model_days
               WHERE strftime('%Y-%m-%d', day, '+0 days') IS NULL
                  OR strftime('%Y-%m-%d', day, '+0 days') != day
                  OR length(day) != 10
                  OR pricing_input_tokens < 0 OR input_tokens < 0
                  OR cached_input_tokens < 0 OR cache_write_input_tokens < 0
                  OR output_tokens < 0 OR reasoning_output_tokens < 0
                  OR observed_tokens < 0 OR complete NOT IN (0, 1)
             ) OR EXISTS(
               SELECT 1 FROM codex_usage_file_days
               WHERE strftime('%Y-%m-%d', day, '+0 days') IS NULL
                  OR strftime('%Y-%m-%d', day, '+0 days') != day
                  OR length(day) != 10
                  OR observed_tokens < 0 OR priced_tokens < 0 OR cost_usd < 0
                  OR complete NOT IN (0, 1)
             ) OR EXISTS(
               SELECT 1 FROM codex_usage_files
               WHERE deferred_until_day IS NOT NULL AND (
                 strftime('%Y-%m-%d', deferred_until_day, '+0 days') IS NULL
                 OR strftime('%Y-%m-%d', deferred_until_day, '+0 days')
                    != deferred_until_day
                 OR length(deferred_until_day) != 10
               )
             )",
        ),
        (
            "claude-usage-values",
            "SELECT EXISTS(
               SELECT 1 FROM claude_usage_messages
               WHERE strftime('%Y-%m-%d', day, '+0 days') IS NULL
                  OR strftime('%Y-%m-%d', day, '+0 days') != day
                  OR length(day) != 10
                  OR input_tokens < 0 OR cache_creation_input_tokens < 0
                  OR cache_read_input_tokens < 0 OR output_tokens < 0
                  OR observed_tokens < 0 OR has_unknown_paid_server_tool NOT IN (0, 1)
                  OR complete NOT IN (0, 1)
             ) OR EXISTS(
               SELECT 1 FROM claude_usage_frames
               WHERE strftime('%Y-%m-%d', day, '+0 days') IS NULL
                  OR strftime('%Y-%m-%d', day, '+0 days') != day
                  OR length(day) != 10
             ) OR EXISTS(
               SELECT 1 FROM claude_usage_daily
               WHERE strftime('%Y-%m-%d', day, '+0 days') IS NULL
                  OR strftime('%Y-%m-%d', day, '+0 days') != day
                  OR length(day) != 10
                  OR observed_tokens < 0 OR priced_tokens < 0
                  OR cost_usd < 0 OR coverage NOT IN ('complete', 'partial')
                  OR revision < 1
             )",
        ),
        (
            "claude-correction-edges",
            "SELECT EXISTS(
               SELECT 1
               FROM claude_usage_message_supersedes AS edge
               LEFT JOIN claude_usage_frames AS replacement
                 ON replacement.frame_key = edge.replacement_frame_key
               LEFT JOIN claude_usage_messages AS superseded
                 ON superseded.frame_key = edge.superseded_frame_key
               LEFT JOIN claude_usage_frames AS superseded_frame
                 ON superseded_frame.frame_key = edge.superseded_frame_key
               WHERE edge.replacement_frame_key = edge.superseded_frame_key
                  OR replacement.frame_key IS NULL
                  OR superseded.frame_key IS NULL
                  OR superseded_frame.frame_key IS NULL
                  OR edge.parser_version != replacement.parser_version
                  OR edge.parser_version != superseded.parser_version
                  OR edge.parser_version != superseded_frame.parser_version
             ) OR EXISTS(
               SELECT 1
               FROM claude_usage_messages AS message
               LEFT JOIN claude_usage_frames AS frame
                 ON frame.frame_key = message.frame_key
               WHERE frame.frame_key IS NULL
                  OR frame.day != message.day
                  OR frame.observed_at != message.observed_at
                  OR frame.parser_version != message.parser_version
             )",
        ),
        (
            "usage-retention",
            "SELECT
               COALESCE((
                 SELECT julianday(MAX(day)) - julianday(MIN(day)) > 29
                 FROM codex_usage_file_days
               ), 0)
               OR COALESCE((
                 SELECT julianday(MAX(day)) - julianday(MIN(day)) > 29
                 FROM codex_usage_file_model_days
               ), 0)
               OR COALESCE((
                 SELECT julianday(MAX(day)) - julianday(MIN(day)) > 59
                 FROM claude_usage_daily
               ), 0)
               OR COALESCE((
                 SELECT julianday(MAX(day)) - julianday(MIN(day)) > 59
                 FROM claude_usage_messages
               ), 0)
               OR COALESCE((
                 SELECT julianday(MAX(day)) - julianday(MIN(day)) > 59
                 FROM claude_usage_frames
               ), 0)",
        ),
    ] {
        let invalid = connection
            .query_row(query, [], |row| row.get::<_, bool>(0))
            .map_err(|_| DatabaseOpenError::InvariantFailed { invariant })?;
        if invalid {
            return Err(DatabaseOpenError::InvariantFailed { invariant });
        }
    }
    Ok(())
}

fn cleanup_module_backups(path: &Path) -> Result<(), DatabaseOpenError> {
    let mut backups = Vec::new();
    for version in 0..=lifecycle::DATABASE_SCHEMA_VERSION {
        backups.push(path.with_extension(format!("sqlite3.backup-v{version}")));
        backups.push(path.with_extension(format!("sqlite3.backup-v{version}.partial")));
    }
    for version in 0..=sanitized::READ_MODEL_SCHEMA_VERSION {
        backups.push(path.with_extension(format!("sqlite3.read-model-v{version}.backup")));
        backups.push(path.with_extension(format!("sqlite3.read-model-v{version}.backup.partial")));
    }
    for version in 0..=providers::CODEX_USAGE_SCHEMA_VERSION {
        backups.push(path.with_extension(format!("sqlite3.codex-usage-v{version}.backup")));
        backups.push(path.with_extension(format!("sqlite3.codex-usage-v{version}.backup.partial")));
    }
    for version in 0..=providers::CLAUDE_USAGE_SCHEMA_VERSION {
        backups.push(path.with_extension(format!("sqlite3.claude-usage-v{version}.backup")));
        backups
            .push(path.with_extension(format!("sqlite3.claude-usage-v{version}.backup.partial")));
    }
    for version in 0..=updater::DATABASE_SCHEMA_VERSION {
        backups.push(path.with_extension(format!("sqlite3.update-state-v{version}.backup")));
        backups
            .push(path.with_extension(format!("sqlite3.update-state-v{version}.backup.partial")));
    }
    for backup in backups {
        match fs::remove_file(backup) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(_) => {
                return Err(DatabaseOpenError::MigrationFailed {
                    stage: "prune-module-backups",
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod release_compatibility;

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    use rusqlite::Connection;

    use super::*;

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
                ("claude-usage-index".to_owned(), 4),
                ("codex-usage-index".to_owned(), 3),
                ("database-coordinator".to_owned(), 1),
                ("desktop-lifecycle".to_owned(), 5),
                ("sanitized-desktop-state".to_owned(), 5),
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
            DatabaseOpenError::InvariantFailed {
                invariant: "index-definitions"
            }
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
}
