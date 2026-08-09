use std::{fs, io::ErrorKind, path::Path};

use rusqlite::{Connection, OpenFlags, params};

use crate::{lifecycle, providers, sanitized, updater};

use super::{
    DatabaseOpenError,
    catalog::{
        COORDINATOR_SCHEMA_MODULE, COORDINATOR_SCHEMA_VERSION, DATABASE_FORMAT_VERSION, INDEXES,
        KNOWN_OBJECT_DEFINITIONS, MODULES, TABLE_COLUMNS, TABLES, VIEWS, normalize_sql,
    },
};

#[derive(Clone, Copy, Debug)]
pub(super) struct SourceInspection {
    pub(super) has_content: bool,
    pub(super) needs_migration: bool,
}

#[derive(Clone, Copy, Debug)]
struct InspectedModuleVersions {
    lifecycle: i64,
    read_model: i64,
    codex: i64,
    claude: i64,
    update: i64,
    has_legacy_update_table: bool,
}

pub(super) fn inspect_source(path: &Path) -> Result<SourceInspection, DatabaseOpenError> {
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

pub(super) fn inspect_registered_modules(connection: &Connection) -> Result<(), DatabaseOpenError> {
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
    inspect_known_object_definitions(
        connection,
        InspectedModuleVersions {
            lifecycle: lifecycle_version,
            read_model: read_model_version,
            codex: codex_version,
            claude: claude_version,
            update: update_version,
            has_legacy_update_table: has_update_table,
        },
    )?;
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

pub(super) fn open_read_only(
    path: &Path,
    stage: &'static str,
) -> Result<Connection, DatabaseOpenError> {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|_| DatabaseOpenError::MigrationFailed { stage })
}

pub(super) fn read_version_rows(
    connection: &Connection,
) -> Result<Vec<(String, i64)>, DatabaseOpenError> {
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

pub(super) fn reject_unknown_objects(connection: &Connection) -> Result<(), DatabaseOpenError> {
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

fn inspect_known_object_definitions(
    connection: &Connection,
    versions: InspectedModuleVersions,
) -> Result<(), DatabaseOpenError> {
    let mut expected_objects = Vec::new();
    if table_exists(connection, "touchgrassbar_schema_versions")? {
        expected_objects.push(("table", "touchgrassbar_schema_versions"));
    }
    if versions.lifecycle >= 4 {
        expected_objects.push(("table", "lifecycle_state"));
    }
    if versions.lifecycle >= 5 {
        expected_objects.push(("table", "provider_settings"));
    }
    if versions.read_model > 0 {
        expected_objects.push(("table", "sanitized_desktop_state"));
    }
    if versions.codex > 0 {
        expected_objects.extend([
            ("table", "codex_usage_index_meta"),
            ("table", "codex_account_usage_meta"),
            ("table", "codex_account_usage_days"),
            ("table", "codex_usage_files"),
            ("table", "codex_usage_file_model_days"),
            ("table", "codex_usage_file_days"),
            ("index", "codex_usage_model_days_by_day"),
            ("index", "codex_usage_unpriced_model_days"),
        ]);
    }
    if versions.claude > 0 {
        expected_objects.extend([
            ("table", "claude_usage_index_meta"),
            ("table", "claude_usage_files"),
            ("table", "claude_usage_messages"),
            ("table", "claude_usage_frames"),
            ("table", "claude_usage_message_supersedes"),
            ("table", "claude_usage_daily"),
            ("index", "claude_usage_messages_by_day"),
            ("index", "claude_usage_messages_by_message"),
            ("index", "claude_usage_messages_by_superseded_frame"),
            ("index", "claude_usage_frames_by_day"),
            ("index", "claude_usage_supersedes_by_superseded_frame"),
        ]);
    }
    match versions.update {
        0 if versions.has_legacy_update_table => {
            expected_objects.push(("table", "touchgrassbar_update_state"));
        }
        1 | 2 => {
            expected_objects.push(("table", "touchgrassbar_update_state"));
        }
        3 => {
            expected_objects.extend([
                ("table", "touchgrassbar_update_state_v3"),
                ("view", "touchgrassbar_update_state"),
            ]);
        }
        _ => {}
    }
    expected_objects.sort_unstable();

    let mut statement = connection
        .prepare(
            "SELECT type, name, sql FROM sqlite_schema
             WHERE type IN ('table', 'index', 'trigger', 'view')
               AND name NOT LIKE 'sqlite_%'
             ORDER BY type, name",
        )
        .map_err(|_| DatabaseOpenError::MigrationFailed {
            stage: "inspect-object-definitions",
        })?;
    let objects = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|_| DatabaseOpenError::MigrationFailed {
            stage: "inspect-object-definitions",
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| DatabaseOpenError::MigrationFailed {
            stage: "inspect-object-definitions",
        })?;
    let stored_objects = objects
        .iter()
        .map(|(object_type, name, _)| (object_type.as_str(), name.as_str()))
        .collect::<Vec<_>>();
    let definitions_are_known = objects.iter().all(|(object_type, name, definition)| {
        let definition = normalize_sql(definition);
        let definition_is_known = KNOWN_OBJECT_DEFINITIONS.contains(&definition.as_str());
        let definition_matches_version = match (object_type.as_str(), name.as_str()) {
            ("table", "sanitized_desktop_state") => match versions.read_model {
                4 => definition.contains("check(schema_version=4)"),
                5 => definition.contains("check(schema_version=5)"),
                _ => false,
            },
            ("table", "touchgrassbar_update_state") => match versions.update {
                0 => definition.contains("deferred_versiontext"),
                1 => {
                    definition.contains("offered_versiontext")
                        && !definition.contains("automatic_checks_enabled")
                }
                2 => definition.contains("automatic_checks_enabled"),
                _ => false,
            },
            ("table", "touchgrassbar_update_state_v3") | ("view", "touchgrassbar_update_state") => {
                versions.update == 3
            }
            _ => true,
        };
        definition_is_known && definition_matches_version
    });
    if stored_objects != expected_objects || !definitions_are_known {
        return Err(DatabaseOpenError::MigrationFailed {
            stage: "inspect-object-definitions",
        });
    }
    Ok(())
}
