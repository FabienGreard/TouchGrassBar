use std::{env, path::PathBuf, process::ExitCode};

fn required_path(name: &str) -> Result<PathBuf, String> {
    env::var_os(name)
        .map(PathBuf::from)
        .ok_or_else(|| format!("{name} is not configured"))
}

fn run() -> Result<(), String> {
    let probe_directory = required_path("TOUCHGRASS_CLAUDE_DEBUG_DIRECTORY")?;
    let report =
        touchgrassbar_lib::run_claude_quota_debug_pass(&probe_directory).map_err(str::to_owned)?;
    eprintln!("{report}");
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("[TouchGrassBar][claude-quota-report] debug_failed reason={error}");
            ExitCode::FAILURE
        }
    }
}
