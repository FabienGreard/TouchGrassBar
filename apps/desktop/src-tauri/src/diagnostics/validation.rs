pub(super) fn database_module(value: &str) -> bool {
    matches!(
        value,
        "database-coordinator"
            | "desktop-lifecycle"
            | "sanitized-desktop-state"
            | "codex-usage-index"
            | "claude-usage-index"
            | "update-state"
            | "unregistered-module"
    )
}

pub(super) fn database_stage(value: &str) -> bool {
    database_module(value)
        || matches!(
            value,
            "database-format"
                | "unregistered-object"
                | "open-lifecycle"
                | "open-native-core"
                | "open-database"
                | "open-ready"
                | "open-source"
                | "open-backup"
                | "open-backup-source"
                | "inspect-source"
                | "inspect-format"
                | "inspect-version-vector"
                | "inspect-desktop-lifecycle"
                | "inspect-sanitized-state"
                | "inspect-codex-usage"
                | "inspect-claude-usage"
                | "inspect-update-state"
                | "inspect-table-columns"
                | "inspect-objects"
                | "inspect-object-definitions"
                | "after-backup"
                | "provider-usage-indexes"
                | "open-final"
                | "configure-final"
                | "begin-final"
                | "write-version-vector"
                | "write-database-format"
                | "before-final-commit"
                | "commit-final"
                | "after-final-commit"
                | "replace-partial-backup"
                | "copy-backup"
                | "before-backup-complete"
                | "sync-backup"
                | "publish-backup"
                | "sync-backup-directory"
                | "replace-partial-marker"
                | "write-migration-marker"
                | "publish-migration-marker"
                | "validate-backup"
                | "finish-migration"
                | "validate-backup-source"
                | "prune-module-backups"
                | "sync-marker-directory"
                | "sync-migration-directory"
                | "integrity"
                | "foreign-keys"
                | "version-vector"
                | "object-registry"
                | "table-registry"
                | "index-registry"
                | "view-registry"
                | "sanitized-state"
                | "profile-projection"
                | "table-columns"
                | "table-definitions"
                | "foreign-key-definitions"
                | "index-definitions"
                | "view-definitions"
                | "lifecycle-state"
                | "provider-settings"
                | "usage-sync-values"
                | "codex-usage-values"
                | "claude-usage-values"
        )
}

pub(super) fn catalog_version(value: &str) -> bool {
    if let Some(hash) = value.strip_prefix("fnv1a64:") {
        return hash.len() == 16 && hash.bytes().all(lower_hex);
    }
    if let Some(hash) = value.strip_prefix("sha256:") {
        return hash.len() == 64 && hash.bytes().all(lower_hex);
    }
    if value.len() > 96 {
        return false;
    }
    let Some(rest) = value
        .strip_prefix("openai-")
        .or_else(|| value.strip_prefix("anthropic-"))
    else {
        return false;
    };
    let Some(rest) = rest
        .strip_prefix("api-")
        .or_else(|| rest.strip_prefix("standard-"))
    else {
        return false;
    };
    let Some((date, version)) = rest.split_once("-v") else {
        return false;
    };
    super::types::day(date)
        && !version.is_empty()
        && !version.starts_with('0')
        && version.bytes().all(|b| b.is_ascii_digit())
}

fn lower_hex(byte: u8) -> bool {
    byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
}

/// Only bounded provider model identifiers, never a path or arbitrary log text.
pub(crate) fn model_identifier(provider: super::Provider, value: &str) -> bool {
    let family_matches = match provider {
        super::Provider::Codex => {
            value
                .strip_prefix("gpt-")
                .is_some_and(|s| s.starts_with(|c: char| c.is_ascii_digit()))
                || value
                    .strip_prefix('o')
                    .is_some_and(|s| s.starts_with(|c: char| c.is_ascii_digit()))
                || value.starts_with("codex-")
        }
        super::Provider::Claude => value.starts_with("claude-"),
    };
    family_matches
        && (2..=96).contains(&value.len())
        && value.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'.' | b'_')
        })
        && value
            .bytes()
            .last()
            .is_some_and(|b| b.is_ascii_alphanumeric())
}
