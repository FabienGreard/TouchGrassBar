//! Compiled coding-provider identities and local presence detection.

use std::{collections::BTreeSet, env, path::PathBuf};

use schemars::JsonSchema;
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
    let mut candidates = BTreeSet::new();
    if let Some(path) = env::var_os("PATH") {
        for directory in env::split_paths(&path) {
            candidates.insert(directory.join(descriptor.command));
        }
    }
    for directory in ["/opt/homebrew/bin", "/usr/local/bin"] {
        candidates.insert(PathBuf::from(directory).join(descriptor.command));
    }
    candidates.insert(PathBuf::from("/Applications").join(descriptor.application));
    if let Some(home) = env::var_os("HOME") {
        let home = PathBuf::from(home);
        for directory in [".local/bin", ".bun/bin", ".npm-global/bin", ".volta/bin"] {
            candidates.insert(home.join(directory).join(descriptor.command));
        }
        if provider == CodingProvider::Codex
            && let Ok(versions) = std::fs::read_dir(home.join(".nvm/versions/node"))
        {
            candidates.extend(
                versions
                    .flatten()
                    .map(|version| version.path().join("bin/codex")),
            );
        }
        candidates.insert(home.join("Applications").join(descriptor.application));
        if provider == CodingProvider::Claude {
            candidates.insert(home.join(".claude/local/claude"));
        }
    }
    candidates
}

pub(crate) fn detect_provider_presence(provider: CodingProvider) -> ProviderPresenceStatus {
    if provider_candidates(provider)
        .iter()
        .any(|candidate| candidate.is_file() || candidate.is_dir())
    {
        ProviderPresenceStatus::Detected
    } else {
        ProviderPresenceStatus::NotDetected
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
