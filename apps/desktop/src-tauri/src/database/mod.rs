mod catalog;
mod inspection;
mod invariants;
mod migration;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use migration::PrepareFault;

/// This value proves that every registered database module is ready.
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

pub(crate) fn prepare(path: &Path) -> Result<PreparedDatabase, DatabaseOpenError> {
    prepare_with_fault(path, PrepareFault::None)
}

fn prepare_with_fault(
    path: &Path,
    fault: PrepareFault,
) -> Result<PreparedDatabase, DatabaseOpenError> {
    let source = inspection::inspect_source(path)?;
    if source.needs_migration {
        migration::migrate(path, source.has_content, fault)?;
    } else {
        let connection = inspection::open_read_only(path, "open-ready")?;
        invariants::verify_invariants(&connection)?;
        drop(connection);
        migration::validate_coordinator_recovery_state(path)?;
        migration::cleanup_module_backups(path)?;
        migration::finish_coordinator_migration(path)?;
    }

    Ok(PreparedDatabase {
        path: path.to_owned(),
    })
}
