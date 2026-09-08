use crate::providers::CodingProvider;

#[cfg(any(target_os = "macos", test))]
use std::process::{Command, Stdio};

const UNAVAILABLE: &str = "installation guide unavailable";

fn guide_url(provider: CodingProvider) -> &'static str {
    match provider {
        CodingProvider::Claude => "https://docs.anthropic.com/en/docs/claude-code/getting-started",
        CodingProvider::Codex => "https://developers.openai.com/codex/cli/",
    }
}

pub(crate) fn open(provider: CodingProvider) -> Result<(), &'static str> {
    #[cfg(target_os = "macos")]
    {
        let mut command = Command::new("/usr/bin/open");
        command.arg(guide_url(provider));
        run_opener(command)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = guide_url(provider);
        Err(UNAVAILABLE)
    }
}

#[cfg(any(target_os = "macos", test))]
fn run_opener(mut command: Command) -> Result<(), &'static str> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|_| UNAVAILABLE)?
        .success()
        .then_some(())
        .ok_or(UNAVAILABLE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_only_the_selected_provider_guide() {
        for (provider, expected) in [
            (
                CodingProvider::Claude,
                "https://docs.anthropic.com/en/docs/claude-code/getting-started",
            ),
            (
                CodingProvider::Codex,
                "https://developers.openai.com/codex/cli/",
            ),
        ] {
            assert_eq!(guide_url(provider), expected);
        }
        assert!(serde_json::from_str::<CodingProvider>("\"https://example.com\"").is_err());
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn reports_opener_exit_and_launch_failures() {
        assert_eq!(run_opener(Command::new("/usr/bin/true")), Ok(()));
        assert_eq!(run_opener(Command::new("/usr/bin/false")), Err(UNAVAILABLE));
        assert_eq!(run_opener(Command::new("/dev/null")), Err(UNAVAILABLE));
    }
}
