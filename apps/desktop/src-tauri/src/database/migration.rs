use std::{
    fs,
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
    time::Duration,
};

use rusqlite::{Connection, params};

use crate::{lifecycle, providers, sanitized, updater};

use super::{
    DatabaseOpenError,
    catalog::{
        COORDINATOR_SCHEMA_MODULE, COORDINATOR_SCHEMA_VERSION, DATABASE_FORMAT_VERSION, MODULES,
    },
    inspection::{
        inspect_registered_modules, open_read_only, read_version_rows, reject_unknown_objects,
    },
    invariants::verify_invariants,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PrepareFault {
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

pub(super) fn migrate(
    path: &Path,
    has_content: bool,
    fault: PrepareFault,
) -> Result<(), DatabaseOpenError> {
    if has_content {
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
    finish_coordinator_migration(path)
}

pub(super) fn coordinator_backup_path(path: &Path) -> PathBuf {
    path.with_extension("sqlite3.compatibility.backup")
}

pub(super) fn coordinator_backup_partial_path(path: &Path) -> PathBuf {
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

pub(super) fn validate_coordinator_recovery_state(path: &Path) -> Result<(), DatabaseOpenError> {
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

pub(super) fn finish_coordinator_migration(path: &Path) -> Result<(), DatabaseOpenError> {
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

pub(super) fn validate_coordinator_backup(path: &Path) -> Result<(), DatabaseOpenError> {
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

pub(super) fn cleanup_module_backups(path: &Path) -> Result<(), DatabaseOpenError> {
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
