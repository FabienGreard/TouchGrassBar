use std::{
    env,
    fs::File,
    io::{self, BufRead, BufReader, Write},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use serde_json::Value;
use tauri::{AppHandle, Manager};
use tauri_plugin_autostart::ManagerExt as AutostartManagerExt;

use crate::sanitized::{
    RefreshAttempt, RefreshFailure, RefreshSource, SanitizedDesktopStateV3, SnapshotRefreshAdapter,
    SnapshotRefreshOutcome,
};

const DRIVER_ARGUMENT: &str = "--touchgrass-release-gates";
const FIXTURE_ENVIRONMENT_NAME: &str = "TOUCHGRASS_RELEASE_REFRESH_FIXTURE";
const PROTOCOL_PREFIX: &str = "touchgrassbar_release_gate event=";
const REFRESH_FIXTURE_VERSION: &str = "touchgrass.refresh-fixture.v1";

#[derive(Clone, Default)]
pub(crate) struct ReleaseGateDriver {
    enabled: bool,
    fixture_path: Option<PathBuf>,
    refresh_active: Arc<AtomicBool>,
    refresh_panel_requested: Arc<AtomicBool>,
    refresh_succeeded: Arc<AtomicBool>,
    renderer_ready: Arc<AtomicBool>,
    tray_ready: Arc<AtomicBool>,
    ready_emitted: Arc<AtomicBool>,
}

impl ReleaseGateDriver {
    pub(crate) fn from_process() -> Self {
        let enabled = env::args_os().any(|argument| argument == DRIVER_ARGUMENT);
        let fixture_path = if enabled {
            env::var_os(FIXTURE_ENVIRONMENT_NAME).map(PathBuf::from)
        } else {
            None
        };
        Self {
            enabled,
            fixture_path,
            refresh_active: Arc::new(AtomicBool::new(false)),
            refresh_panel_requested: Arc::new(AtomicBool::new(false)),
            refresh_succeeded: Arc::new(AtomicBool::new(false)),
            renderer_ready: Arc::new(AtomicBool::new(false)),
            tray_ready: Arc::new(AtomicBool::new(false)),
            ready_emitted: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(crate) fn enabled(&self) -> bool {
        self.enabled
    }

    pub(crate) fn refresh_adapter(&self) -> Arc<dyn SnapshotRefreshAdapter> {
        Arc::new(ReleaseFixtureRefreshAdapter {
            fixture_path: self.fixture_path.clone(),
            panel_requested: Arc::clone(&self.refresh_panel_requested),
            succeeded: Arc::clone(&self.refresh_succeeded),
        })
    }

    pub(crate) fn start(&self, app: AppHandle) -> io::Result<()> {
        if !self.enabled {
            return Ok(());
        }
        self.tray_ready.store(true, Ordering::Release);
        self.emit_ready_if_complete();
        let driver = self.clone();
        thread::Builder::new()
            .name("release-gate-driver".to_owned())
            .spawn(move || driver.command_loop(app))?;
        Ok(())
    }

    pub(crate) fn renderer_ready(&self) {
        if !self.enabled {
            return;
        }
        self.renderer_ready.store(true, Ordering::Release);
        self.emit_ready_if_complete();
    }

    fn emit_ready_if_complete(&self) {
        if self.tray_ready.load(Ordering::Acquire)
            && self.renderer_ready.load(Ordering::Acquire)
            && !self.ready_emitted.swap(true, Ordering::AcqRel)
        {
            emit("menu_bar_ready");
        }
    }

    fn command_loop(self, app: AppHandle) {
        for line in BufReader::new(io::stdin().lock()).lines() {
            let Ok(command) = line else {
                emit("driver_failed");
                return;
            };
            match command.as_str() {
                "outside_click" => self.outside_click_preflight(&app),
                "rapid" => self.rapid_interaction_preflight(&app),
                "show" => self.show_panel(&app),
                "hide" => self.hide_panel(&app),
                "launch_at_login" => self.launch_at_login_preflight(&app),
                "refresh" => self.refresh_fixture(&app),
                "quit" => {
                    app.exit(0);
                    return;
                }
                _ => emit("invalid_command"),
            }
        }
    }

    fn show_panel(&self, app: &AppHandle) {
        let refresh_active = Arc::clone(&self.refresh_active);
        let refresh_panel_requested = Arc::clone(&self.refresh_panel_requested);
        let app = app.clone();
        if app
            .clone()
            .run_on_main_thread(move || {
                let Some(tray) = app.tray_by_id("touchgrassbar") else {
                    emit("show_failed");
                    return;
                };
                let Ok(Some(rect)) = tray.rect() else {
                    emit("show_failed");
                    return;
                };
                if refresh_active.load(Ordering::Acquire) {
                    refresh_panel_requested.store(true, Ordering::Release);
                }
                let event = match crate::handle_tray_release(
                    &app,
                    rect,
                    Instant::now(),
                    crate::performance::PanelPaintSource::Synthetic,
                ) {
                    Ok(true) => "show_accepted",
                    Ok(false) => "toggled_hidden",
                    Err(_) => "show_failed",
                };
                emit(event);
            })
            .is_err()
        {
            emit("show_failed");
        }
    }

    fn rapid_interaction_preflight(&self, app: &AppHandle) {
        let app = app.clone();
        if app
            .clone()
            .run_on_main_thread(move || {
                let Some(tray) = app.tray_by_id("touchgrassbar") else {
                    emit("rapid_interaction_failed");
                    return;
                };
                let Ok(Some(rect)) = tray.rect() else {
                    emit("rapid_interaction_failed");
                    return;
                };
                let first = crate::handle_tray_release(
                    &app,
                    rect,
                    Instant::now(),
                    crate::performance::PanelPaintSource::Synthetic,
                );
                let second = crate::handle_tray_release(
                    &app,
                    rect,
                    Instant::now(),
                    crate::performance::PanelPaintSource::Synthetic,
                );
                let third = crate::handle_tray_release(
                    &app,
                    rect,
                    Instant::now(),
                    crate::performance::PanelPaintSource::Synthetic,
                );
                emit(
                    if matches!((first, second, third), (Ok(true), Ok(false), Ok(true))) {
                        "rapid_interaction_pass"
                    } else {
                        "rapid_interaction_failed"
                    },
                );
            })
            .is_err()
        {
            emit("rapid_interaction_failed");
        }
    }

    fn outside_click_preflight(&self, app: &AppHandle) {
        let focus_app = app.clone();
        if app
            .run_on_main_thread(move || {
                let result = focus_app
                    .get_webview_window(crate::SETTINGS_LABEL)
                    .ok_or(())
                    .and_then(|settings| settings.show().map_err(|_| ()).map(|()| settings))
                    .and_then(|settings| settings.set_focus().map_err(|_| ()));
                if result.is_err() {
                    emit("outside_click_failed");
                }
            })
            .is_err()
        {
            emit("outside_click_failed");
            return;
        }

        let verify_app = app.clone();
        let spawn = thread::Builder::new()
            .name("release-gate-outside-click".to_owned())
            .spawn(move || {
                let deadline = Instant::now() + Duration::from_secs(2);
                let hidden = loop {
                    let hidden = verify_app
                        .get_webview_window(crate::PANEL_LABEL)
                        .and_then(|panel| panel.is_visible().ok())
                        == Some(false);
                    if hidden || Instant::now() >= deadline {
                        break hidden;
                    }
                    thread::sleep(Duration::from_millis(10));
                };
                if let Some(settings) = verify_app.get_webview_window(crate::SETTINGS_LABEL) {
                    let _ = settings.hide();
                }
                emit(if hidden {
                    "outside_click_pass"
                } else {
                    "outside_click_failed"
                });
            });
        if spawn.is_err() {
            emit("outside_click_failed");
        }
    }

    fn hide_panel(&self, app: &AppHandle) {
        let app = app.clone();
        if app
            .clone()
            .run_on_main_thread(move || {
                if let Some(probe) = app.try_state::<crate::performance::PanelPaintProbe>() {
                    probe.cancel();
                }
                let result = app
                    .get_webview_window(crate::PANEL_LABEL)
                    .ok_or(())
                    .and_then(|panel| panel.hide().map_err(|_| ()));
                emit(if result.is_ok() {
                    "hidden"
                } else {
                    "hide_failed"
                });
            })
            .is_err()
        {
            emit("hide_failed");
        }
    }

    fn launch_at_login_preflight(&self, app: &AppHandle) {
        let app = app.clone();
        let spawn = thread::Builder::new()
            .name("release-gate-launch-at-login".to_owned())
            .spawn(move || {
                let manager = app.autolaunch();
                let Ok(original) = manager.is_enabled() else {
                    emit("launch_at_login_failed");
                    return;
                };
                let enabled =
                    manager.enable().is_ok() && manager.is_enabled().is_ok_and(|enabled| enabled);
                let disabled =
                    manager.disable().is_ok() && manager.is_enabled().is_ok_and(|enabled| !enabled);
                let restored = if original {
                    manager.enable().is_ok() && manager.is_enabled().is_ok_and(|enabled| enabled)
                } else {
                    manager.disable().is_ok() && manager.is_enabled().is_ok_and(|enabled| !enabled)
                };
                emit(if enabled && disabled && restored {
                    "launch_at_login_pass"
                } else {
                    "launch_at_login_failed"
                });
            });
        if spawn.is_err() {
            emit("launch_at_login_failed");
        }
    }

    fn refresh_fixture(&self, app: &AppHandle) {
        if self.refresh_active.swap(true, Ordering::AcqRel) {
            emit("refresh_failed");
            return;
        }
        if self.fixture_path.is_none() {
            self.refresh_active.store(false, Ordering::Release);
            emit("refresh_failed");
            return;
        }
        self.refresh_panel_requested.store(false, Ordering::Release);
        self.refresh_succeeded.store(false, Ordering::Release);
        let active = Arc::clone(&self.refresh_active);
        let succeeded = Arc::clone(&self.refresh_succeeded);
        let core = app.state::<crate::sanitized::NativeCore>().inner().clone();
        emit("refresh_started");
        let spawn = thread::Builder::new()
            .name("release-gate-local-refresh".to_owned())
            .spawn(move || {
                let result = core
                    .revision_notices()
                    .and_then(|notices| {
                        core.request_refresh(RefreshSource::Manual)?;
                        core.wait_for_refresh_completion()?;
                        notices
                            .recv_timeout(Duration::from_secs(2))
                            .map_err(|_| "refresh completion unavailable")?;
                        Ok(())
                    })
                    .is_ok()
                    && succeeded.load(Ordering::Acquire);
                active.store(false, Ordering::Release);
                emit(if result {
                    "refresh_complete"
                } else {
                    "refresh_failed"
                });
            });
        if spawn.is_err() {
            self.refresh_active.store(false, Ordering::Release);
            emit("refresh_failed");
        }
    }
}

struct ReleaseFixtureRefreshAdapter {
    fixture_path: Option<PathBuf>,
    panel_requested: Arc<AtomicBool>,
    succeeded: Arc<AtomicBool>,
}

impl SnapshotRefreshAdapter for ReleaseFixtureRefreshAdapter {
    fn refresh(
        &self,
        _cached: SanitizedDesktopStateV3,
        attempt: &RefreshAttempt,
    ) -> Result<SnapshotRefreshOutcome, RefreshFailure> {
        self.succeeded.store(false, Ordering::Release);
        if !attempt.is_manual() {
            return Ok(SnapshotRefreshOutcome::default());
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        while !self.panel_requested.load(Ordering::Acquire) {
            attempt.remaining()?;
            if Instant::now() >= deadline {
                return Err(RefreshFailure::SourceUnavailable);
            }
            thread::sleep(Duration::from_millis(1));
        }
        let path = self
            .fixture_path
            .clone()
            .ok_or(RefreshFailure::SourceUnavailable)?;
        let snapshot = load_refresh_fixture(path)?;
        self.succeeded.store(true, Ordering::Release);
        Ok(SnapshotRefreshOutcome {
            snapshot: Some(snapshot),
            completed_providers: Default::default(),
        })
    }
}

fn load_refresh_fixture(path: PathBuf) -> Result<SanitizedDesktopStateV3, RefreshFailure> {
    let fixture: Value =
        serde_json::from_reader(File::open(path).map_err(|_| RefreshFailure::SourceUnavailable)?)
            .map_err(|_| RefreshFailure::SourceUnavailable)?;
    validate_refresh_fixture(&fixture).map_err(|()| RefreshFailure::SourceUnavailable)?;
    serde_json::from_value(
        fixture
            .get("panel_projection")
            .cloned()
            .ok_or(RefreshFailure::SourceUnavailable)?,
    )
    .map_err(|_| RefreshFailure::SourceUnavailable)
}

fn validate_refresh_fixture(fixture: &Value) -> Result<(), ()> {
    let maxima = fixture.get("maxima").ok_or(())?;
    let providers = fixture
        .get("providers")
        .and_then(Value::as_array)
        .ok_or(())?;
    let doomerboards = fixture.get("doomerboards").ok_or(())?;
    let panel_projection = fixture.get("panel_projection").ok_or(())?;
    if fixture.get("version").and_then(Value::as_str) != Some(REFRESH_FIXTURE_VERSION)
        || fixture.get("source").and_then(Value::as_str) != Some("synthetic")
        || maxima.get("supported_providers").and_then(Value::as_u64) != Some(2)
        || maxima
            .get("ranking_days_per_provider")
            .and_then(Value::as_u64)
            != Some(60)
        || maxima
            .get("model_cost_days_per_provider")
            .and_then(Value::as_u64)
            != Some(30)
        || maxima.get("global_rows").and_then(Value::as_u64) != Some(100)
        || maxima.get("my_tokenmaxxers_rows").and_then(Value::as_u64) != Some(100)
        || providers.len() != 2
        || providers.iter().any(|provider| {
            provider
                .get("ranking_days")
                .and_then(Value::as_array)
                .is_none_or(|days| days.len() != 60)
                || provider
                    .get("model_cost_days")
                    .and_then(Value::as_array)
                    .is_none_or(|days| days.len() != 30)
        })
        || doomerboards
            .get("global")
            .and_then(Value::as_array)
            .is_none_or(|rows| rows.len() != 100)
        || panel_projection
            .get("providers")
            .and_then(Value::as_array)
            .is_none_or(|providers| providers.len() != 2)
        || doomerboards
            .get("my_tokenmaxxers")
            .and_then(Value::as_array)
            .is_none_or(|rows| rows.len() != 100)
    {
        return Err(());
    }
    Ok(())
}

fn emit(event: &str) {
    println!("{PROTOCOL_PREFIX}{event}");
    let _ = io::stdout().flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn driver_stays_disabled_without_the_exact_argument() {
        let driver = ReleaseGateDriver::default();
        assert!(!driver.enabled());
    }

    #[test]
    fn maximum_refresh_fixture_uses_the_versioned_synthetic_shape() {
        let fixture = serde_json::json!({
            "version": REFRESH_FIXTURE_VERSION,
            "source": "synthetic",
            "maxima": {
                "supported_providers": 2,
                "ranking_days_per_provider": 60,
                "model_cost_days_per_provider": 30,
                "global_rows": 100,
                "my_tokenmaxxers_rows": 100,
            },
            "providers": [
                { "ranking_days": vec![Value::Null; 60], "model_cost_days": vec![Value::Null; 30] },
                { "ranking_days": vec![Value::Null; 60], "model_cost_days": vec![Value::Null; 30] },
            ],
            "doomerboards": {
                "global": vec![Value::Null; 100],
                "my_tokenmaxxers": vec![Value::Null; 100],
            },
            "panel_projection": crate::sanitized::unavailable_state(1),
        });
        assert_eq!(validate_refresh_fixture(&fixture), Ok(()));
        assert!(
            serde_json::from_value::<SanitizedDesktopStateV3>(fixture["panel_projection"].clone())
                .is_ok()
        );

        let mut incomplete = fixture;
        incomplete["providers"][0]["ranking_days"] = serde_json::json!([]);
        assert_eq!(validate_refresh_fixture(&incomplete), Err(()));
    }
}
