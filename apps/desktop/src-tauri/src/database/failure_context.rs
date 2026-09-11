//! Fixed read-only evidence collected only after a database operation fails.

use std::{
    fs,
    io::ErrorKind,
    path::Path,
    time::{Duration, Instant},
};

use rusqlite::{Connection, OpenFlags, OptionalExtension};

use crate::diagnostics::{BackupState, DatabaseCode, DatabaseContext, DatabaseModule, Failure};

use super::{
    DatabaseOpenError,
    catalog::{DATABASE_FORMAT_VERSION, MODULES},
    migration::coordinator_backup_path,
};

const QUERY_BUDGET: Duration = Duration::from_millis(100);

pub(super) fn failure(path: &Path, error: DatabaseOpenError) -> Failure {
    let code = match error {
        DatabaseOpenError::UnsupportedFuture { .. } => DatabaseCode::DatabaseVersionUnsupported,
        DatabaseOpenError::MigrationFailed { .. } => DatabaseCode::DatabaseMigrationFailed,
        DatabaseOpenError::InvariantFailed { .. } => DatabaseCode::DatabaseInvariantFailed,
    };
    open_failure(Some(path), code, error.detail())
}

pub(crate) fn open_failure(path: Option<&Path>, code: DatabaseCode, stage: &str) -> Failure {
    let mut context = DatabaseContext {
        stage: Some(stage.to_owned()),
        observed_format: None,
        expected_format: Some(DATABASE_FORMAT_VERSION as u64),
        modules: MODULES
            .iter()
            .map(|(module, version)| DatabaseModule {
                module: (*module).to_owned(),
                observed_version: None,
                expected_version: Some(*version as u64),
            })
            .collect(),
        backup_state: path.map_or(BackupState::Unknown, |path| {
            match fs::metadata(coordinator_backup_path(path)) {
                Ok(_) => BackupState::Present,
                Err(error) if error.kind() == ErrorKind::NotFound => BackupState::Absent,
                Err(_) => BackupState::Unknown,
            }
        }),
    };
    // Never create a file, migrate, validate a backup again, or export schema SQL.
    // Unknown names and row contents do not enter the report.
    if let Some(connection) = path
        .and_then(|path| Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).ok())
    {
        let _ = connection.busy_timeout(Duration::from_millis(20));
        let deadline = Instant::now() + QUERY_BUDGET;
        connection.progress_handler(100, Some(move || Instant::now() >= deadline));
        context.observed_format = connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .ok()
            .and_then(safe_version);
        for module in &mut context.modules {
            if Instant::now() >= deadline {
                break;
            }
            module.observed_version = connection
                .query_row(
                    "SELECT version FROM touchgrassbar_schema_versions WHERE module = ?1 LIMIT 1",
                    [&module.module],
                    |row| row.get::<_, i64>(0),
                )
                .optional()
                .ok()
                .flatten()
                .and_then(safe_version);
        }
    }
    Failure::Database {
        code,
        provider: None,
        context,
    }
}

fn safe_version(value: i64) -> Option<u64> {
    u64::try_from(value)
        .ok()
        .filter(|value| *value <= i32::MAX as u64)
}
