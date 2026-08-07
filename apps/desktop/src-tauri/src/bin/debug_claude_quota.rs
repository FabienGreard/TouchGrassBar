use std::{env, path::PathBuf, process::ExitCode};

fn required_path(name: &str) -> Result<PathBuf, String> {
    env::var_os(name)
        .map(PathBuf::from)
        .ok_or_else(|| format!("{name} is not configured"))
}

fn run() -> Result<(), String> {
    let database_path = required_path("TOUCHGRASS_CLAUDE_DEBUG_DATABASE")?;
    let source_database = env::var_os("TOUCHGRASS_CLAUDE_DEBUG_SOURCE_DATABASE")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    let seed_fixture = env::var("TOUCHGRASS_CLAUDE_DEBUG_FIXTURE").as_deref() == Ok("1");
    let report = touchgrassbar_lib::run_claude_quota_debug_pass(
        &database_path,
        source_database.as_deref(),
        seed_fixture,
    )
    .map_err(str::to_owned)?;
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
