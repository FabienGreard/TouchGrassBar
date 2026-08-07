//! Compiled coding-provider identities and local presence detection.

use std::{
    collections::BTreeSet,
    env,
    path::{Path, PathBuf},
};

use schemars::JsonSchema;
use semver::Version;
use serde::{Deserialize, Serialize};

#[derive(
    Clone, Copy, Debug, Deserialize, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize,
)]
#[serde(rename_all = "lowercase")]
pub enum CodingProvider {
    Codex,
    Claude,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderPresenceStatus {
    Detected,
    NotDetected,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ProviderDescriptor {
    pub(crate) provider: CodingProvider,
    pub(crate) display_name: &'static str,
    command: &'static str,
    application: &'static str,
}

pub(crate) const PROVIDER_REGISTRY: [ProviderDescriptor; 2] = [
    ProviderDescriptor {
        provider: CodingProvider::Codex,
        display_name: "Codex",
        command: "codex",
        application: "Codex.app",
    },
    ProviderDescriptor {
        provider: CodingProvider::Claude,
        display_name: "Claude",
        command: "claude",
        application: "Claude.app",
    },
];

pub(crate) fn provider_descriptor(provider: CodingProvider) -> &'static ProviderDescriptor {
    PROVIDER_REGISTRY
        .iter()
        .find(|descriptor| descriptor.provider == provider)
        .expect("compiled provider must have a descriptor")
}

pub(crate) fn provider_candidates(provider: CodingProvider) -> BTreeSet<PathBuf> {
    let descriptor = provider_descriptor(provider);
    let mut candidates = provider_executable_candidates(provider)
        .into_iter()
        .collect::<BTreeSet<_>>();
    candidates.insert(PathBuf::from("/Applications").join(descriptor.application));
    if let Some(home) = env::var_os("HOME") {
        candidates.insert(
            PathBuf::from(home)
                .join("Applications")
                .join(descriptor.application),
        );
    }
    candidates
}

fn provider_executable_candidates(provider: CodingProvider) -> Vec<PathBuf> {
    let descriptor = provider_descriptor(provider);
    let mut candidates = Vec::new();
    let mut seen = BTreeSet::new();
    let mut push = |candidate: PathBuf| {
        if seen.insert(candidate.clone()) {
            candidates.push(candidate);
        }
    };
    if let Some(path) = env::var_os("PATH") {
        for directory in env::split_paths(&path) {
            push(directory.join(descriptor.command));
        }
    }
    for directory in ["/opt/homebrew/bin", "/usr/local/bin"] {
        push(PathBuf::from(directory).join(descriptor.command));
    }
    if let Some(home) = env::var_os("HOME") {
        let home = PathBuf::from(home);
        for candidate in home_provider_executable_candidates(provider, &home) {
            push(candidate);
        }
    }
    candidates
}

fn home_provider_executable_candidates(provider: CodingProvider, home: &Path) -> Vec<PathBuf> {
    let descriptor = provider_descriptor(provider);
    let mut candidates = Vec::new();
    for directory in [".local/bin", ".bun/bin", ".npm-global/bin", ".volta/bin"] {
        candidates.push(home.join(directory).join(descriptor.command));
    }
    if let Ok(versions) = std::fs::read_dir(home.join(".nvm/versions/node")) {
        let mut version_candidates = versions
            .flatten()
            .map(|entry| {
                let version = entry
                    .file_name()
                    .to_str()
                    .and_then(|name| name.strip_prefix('v'))
                    .and_then(|name| Version::parse(name).ok());
                (version, entry.path().join("bin").join(descriptor.command))
            })
            .collect::<Vec<_>>();
        // Try valid NVM versions from newest to oldest. Keep other entries as fallbacks.
        version_candidates.sort_by(|(left_version, left_path), (right_version, right_path)| {
            right_version
                .cmp(left_version)
                .then_with(|| right_path.cmp(left_path))
        });
        candidates.extend(
            version_candidates
                .into_iter()
                .map(|(_, candidate)| candidate),
        );
    }
    if provider == CodingProvider::Claude {
        candidates.push(home.join(".claude/local/claude"));
    }
    candidates
}

pub(crate) fn resolve_provider_executable(provider: CodingProvider) -> Option<PathBuf> {
    first_executable(provider_executable_candidates(provider))
}

fn first_executable(candidates: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    candidates
        .into_iter()
        .find(|candidate| is_executable_file(candidate))
}

fn is_executable_file(candidate: &Path) -> bool {
    let Ok(metadata) = candidate.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

pub(crate) fn detect_provider_presence(provider: CodingProvider) -> ProviderPresenceStatus {
    if provider_candidates(provider)
        .iter()
        .any(|candidate| is_executable_file(candidate) || candidate.is_dir())
    {
        ProviderPresenceStatus::Detected
    } else {
        ProviderPresenceStatus::NotDetected
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        sync::atomic::{AtomicU64, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::*;

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

    struct FixtureHome(PathBuf);

    impl FixtureHome {
        fn new() -> Self {
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            Self(env::temp_dir().join(format!(
                "touchgrassbar-provider-registry-{}-{timestamp}-{}",
                std::process::id(),
                NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
            )))
        }
    }

    impl Drop for FixtureHome {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn registry_ids_and_display_names_are_unique() {
        let ids = PROVIDER_REGISTRY
            .iter()
            .map(|descriptor| descriptor.provider)
            .collect::<BTreeSet<_>>();
        let names = PROVIDER_REGISTRY
            .iter()
            .map(|descriptor| descriptor.display_name)
            .collect::<BTreeSet<_>>();
        assert_eq!(ids.len(), PROVIDER_REGISTRY.len());
        assert_eq!(names.len(), PROVIDER_REGISTRY.len());
    }

    #[test]
    fn nvm_candidates_include_claude_and_codex() {
        let home = FixtureHome::new();
        let version_bin = home.0.join(".nvm/versions/node/v24.0.0/bin");
        fs::create_dir_all(&version_bin).unwrap();

        assert!(
            home_provider_executable_candidates(CodingProvider::Claude, &home.0)
                .contains(&version_bin.join("claude"))
        );
        assert!(
            home_provider_executable_candidates(CodingProvider::Codex, &home.0)
                .contains(&version_bin.join("codex"))
        );
    }

    #[test]
    fn nvm_candidates_use_descending_semantic_versions() {
        let home = FixtureHome::new();
        let versions_root = home.0.join(".nvm/versions/node");
        for version in ["v9.0.0", "v20.10.0", "v24.0.0", "not-a-version"] {
            fs::create_dir_all(versions_root.join(version)).unwrap();
        }

        let candidates = home_provider_executable_candidates(CodingProvider::Claude, &home.0)
            .into_iter()
            .filter(|candidate| candidate.starts_with(&versions_root))
            .collect::<Vec<_>>();

        assert_eq!(
            candidates,
            ["v24.0.0", "v20.10.0", "v9.0.0", "not-a-version"]
                .map(|version| versions_root.join(version).join("bin/claude"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn executable_resolution_preserves_priority_and_skips_stale_files() {
        use std::os::unix::fs::PermissionsExt;

        let home = FixtureHome::new();
        fs::create_dir_all(&home.0).unwrap();
        let stale = home.0.join("stale");
        let preferred = home.0.join("preferred");
        let fallback = home.0.join("fallback");
        for candidate in [&stale, &preferred, &fallback] {
            fs::write(candidate, b"fixture").unwrap();
        }
        fs::set_permissions(&stale, fs::Permissions::from_mode(0o600)).unwrap();
        fs::set_permissions(&preferred, fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(&fallback, fs::Permissions::from_mode(0o700)).unwrap();

        assert_eq!(
            first_executable([stale, preferred.clone(), fallback]),
            Some(preferred)
        );
    }
}
