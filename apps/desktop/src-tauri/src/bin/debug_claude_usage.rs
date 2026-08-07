use std::{
    env,
    path::{Component, PathBuf},
    process::ExitCode,
};

fn validate_path(path: PathBuf) -> Result<PathBuf, String> {
    let valid = !path.as_os_str().is_empty()
        && path.is_absolute()
        && path.parent().is_some()
        && !path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir));
    valid
        .then_some(path)
        .ok_or_else(|| "The configured path is unsafe".to_owned())
}

fn required_path(name: &str) -> Result<PathBuf, String> {
    env::var_os(name)
        .map(PathBuf::from)
        .ok_or_else(|| format!("{name} is not configured"))
        .and_then(validate_path)
}

fn parse_pass_count(value: Option<&str>) -> Result<usize, String> {
    let passes = value
        .unwrap_or("1")
        .parse::<usize>()
        .map_err(|_| "The pass count is invalid".to_owned())?;
    if !(1..=100).contains(&passes) {
        return Err("The pass count must be from 1 through 100".to_owned());
    }
    Ok(passes)
}

fn pass_count() -> Result<usize, String> {
    parse_pass_count(
        env::var("TOUCHGRASS_CLAUDE_USAGE_DEBUG_PASSES")
            .ok()
            .as_deref(),
    )
}

fn run() -> Result<(), String> {
    let database_path = required_path("TOUCHGRASS_CLAUDE_USAGE_DEBUG_DATABASE")?;
    let config_root = required_path("TOUCHGRASS_CLAUDE_USAGE_CONFIG_ROOT")?;
    let probe_directory = required_path("TOUCHGRASS_CLAUDE_USAGE_PROBE_DIRECTORY")?;
    for pass in 1..=pass_count()? {
        eprintln!("[TouchGrassBar][claude-usage] debug_pass_started pass={pass}");
        let report = touchgrassbar_lib::run_claude_usage_debug_pass(
            &database_path,
            &config_root,
            &probe_directory,
        )
        .map_err(str::to_owned)?;
        eprintln!("{report}");
        eprintln!("[TouchGrassBar][claude-usage] debug_pass_completed pass={pass}");
    }
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("[TouchGrassBar][claude-usage] debug_failed reason={error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{parse_pass_count, validate_path};

    #[test]
    fn pass_count_accepts_the_default_and_bounds() {
        assert_eq!(parse_pass_count(None), Ok(1));
        assert_eq!(parse_pass_count(Some("1")), Ok(1));
        assert_eq!(parse_pass_count(Some("100")), Ok(100));
    }

    #[test]
    fn pass_count_rejects_invalid_values() {
        assert!(parse_pass_count(Some("0")).is_err());
        assert!(parse_pass_count(Some("101")).is_err());
        assert!(parse_pass_count(Some("many")).is_err());
    }

    #[test]
    fn path_gate_accepts_a_normal_absolute_path() {
        let path = std::env::temp_dir().join("touchgrassbar-claude-usage-debug");
        assert_eq!(validate_path(path.clone()), Ok(path));
    }

    #[test]
    fn path_gate_rejects_empty_relative_root_and_parent_paths() {
        assert!(validate_path(PathBuf::new()).is_err());
        assert!(validate_path(PathBuf::from("relative/path")).is_err());
        assert!(validate_path(PathBuf::from(std::path::MAIN_SEPARATOR_STR)).is_err());
        assert!(
            validate_path(
                std::env::temp_dir()
                    .join("private")
                    .join("..")
                    .join("index")
            )
            .is_err()
        );
    }
}
