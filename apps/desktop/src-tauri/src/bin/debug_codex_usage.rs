use std::{env, path::PathBuf, process::ExitCode, time::Instant};

fn required_path(name: &str) -> Result<PathBuf, String> {
    env::var_os(name)
        .map(PathBuf::from)
        .ok_or_else(|| format!("{name} is not configured"))
}

fn pass_count() -> Result<usize, String> {
    let value = env::var("TOUCHGRASS_USAGE_DEBUG_PASSES").unwrap_or_else(|_| "1".to_owned());
    let passes = value
        .parse::<usize>()
        .map_err(|_| "The pass count is invalid".to_owned())?;
    if !(1..=100).contains(&passes) {
        return Err("The pass count must be from 1 through 100".to_owned());
    }
    Ok(passes)
}

fn run() -> Result<(), String> {
    let database_path = required_path("TOUCHGRASS_USAGE_DEBUG_DATABASE")?;
    let codex_home = required_path("CODEX_HOME")?;
    for pass in 1..=pass_count()? {
        let started = Instant::now();
        eprintln!("[TouchGrassBar][codex-usage] debug_pass_started pass={pass}");
        let report = touchgrassbar_lib::run_codex_usage_debug_pass(&database_path, &codex_home)
            .map_err(str::to_owned)?;
        eprintln!("{report}");
        eprintln!(
            "[TouchGrassBar][codex-usage] debug_pass_completed pass={pass} elapsed_ms={}",
            started.elapsed().as_millis()
        );
    }
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("[TouchGrassBar][codex-usage] debug_failed reason={error}");
            ExitCode::FAILURE
        }
    }
}
