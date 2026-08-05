#[cfg(debug_assertions)]
mod dev_instance;
pub mod lifecycle;
mod network;
pub mod profile;
pub mod sanitized;

use std::{
    env,
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};

use lifecycle::{
    BootstrapStateV2, DesktopLifecycle, LaunchAtLoginState, SETTINGS_NAVIGATION_EVENT,
    SettingsNavigationRequest, SettingsProfileAuthorization, SettingsSection, SettingsStateV2,
};
use sanitized::{
    NativeCore, REVISION_NOTICE_EVENT, RefreshReceipt, RefreshSource, RevisionNotice,
    SanitizedDesktopStateV2, SanitizedProfileOutcome,
};
use tauri::{
    ActivationPolicy, AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, PhysicalSize,
    Position, Rect, RunEvent, Size, State, WebviewWindow,
    menu::{MenuBuilder, MenuItemBuilder, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};
use tauri_plugin_autostart::MacosLauncher;
use tauri_plugin_autostart::ManagerExt as AutostartManagerExt;

const PANEL_LABEL: &str = "panel";
const SETTINGS_LABEL: &str = "settings";
const ONBOARDING_LABEL: &str = "onboarding";
const PANEL_WIDTH: f64 = 402.0;
const MIN_PANEL_HEIGHT: f64 = 320.0;
const MAX_PANEL_HEIGHT: f64 = 720.0;
const MENU_BAR_ICON: &[u8] =
    include_bytes!("../../../../packages/ui/src/assets/brand/grass-glyph-white.png");

pub(crate) fn profile_attempt_metric<T, E>(attempt: &Result<T, E>) -> &'static str {
    match attempt {
        Ok(_) => "touchgrassbar_metric profile_attempt=complete",
        Err(_) => "touchgrassbar_metric profile_attempt=pending",
    }
}

#[derive(Clone)]
struct ProfileRetryMailbox {
    pending: Arc<Mutex<bool>>,
    wake: mpsc::SyncSender<()>,
}

impl ProfileRetryMailbox {
    fn new() -> (Self, mpsc::Receiver<()>) {
        let (wake, receiver) = mpsc::sync_channel(1);
        (
            Self {
                pending: Arc::new(Mutex::new(false)),
                wake,
            },
            receiver,
        )
    }

    fn request(&self) {
        let Ok(mut pending) = self.pending.lock() else {
            return;
        };
        *pending = true;
        let _ = self.wake.try_send(());
    }

    fn take(&self) -> bool {
        self.pending
            .lock()
            .is_ok_and(|mut pending| std::mem::take(&mut *pending))
    }
}

#[derive(Clone)]
struct ProfileRuntime {
    coordinator: Arc<std::sync::Mutex<profile::ProfileCoordinator>>,
    core: NativeCore,
    lifecycle: DesktopLifecycle,
    retry: ProfileRetryMailbox,
}

impl ProfileRuntime {
    #[cfg(target_os = "macos")]
    fn start(lifecycle: DesktopLifecycle, app: AppHandle) -> std::io::Result<Self> {
        let runtime_lifecycle = lifecycle.clone();
        let coordinator = Arc::new(std::sync::Mutex::new(profile::production_coordinator(
            lifecycle,
        )));
        let (retry, requests) = ProfileRetryMailbox::new();
        let worker_retry = retry.clone();
        let runtime = Self {
            coordinator,
            core: app.state::<NativeCore>().inner().clone(),
            lifecycle: runtime_lifecycle,
            retry,
        };
        let worker_runtime = runtime.clone();
        std::thread::Builder::new()
            .name("profile-provisioning-retry".to_owned())
            .spawn(move || {
                loop {
                    match requests.recv_timeout(Duration::from_secs(300)) {
                        Ok(()) => {
                            if !worker_retry.take() {
                                continue;
                            }
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                    let _ = worker_runtime.attempt();
                }
            })?;
        Ok(runtime)
    }

    fn trigger(&self) {
        self.retry.request();
    }

    fn attempt(&self) -> Result<Option<SanitizedProfileOutcome>, String> {
        let attempt = self
            .coordinator
            .lock()
            .map_err(|_| "Profile Pending".to_owned())?
            .retry_pending()
            .map_err(|_| "Profile Pending".to_owned());
        eprintln!("{}", profile_attempt_metric(&attempt));
        let profile = attempt?;
        if let Some(profile) = &profile {
            self.core
                .set_profile_outcome(profile.clone())
                .map_err(str::to_owned)?;
        }
        Ok(profile)
    }

    fn attempt_now(&self) -> Result<Option<SanitizedProfileOutcome>, String> {
        self.attempt()
    }

    fn reveal_recovery_key(
        &self,
        authorization: SettingsProfileAuthorization,
    ) -> Result<String, String> {
        self.coordinator
            .lock()
            .map_err(|_| "Recovery Key unavailable".to_owned())?
            .recovery_key(authorization)
            .map(|key| key.expose().to_owned())
            .map_err(|_| "Recovery Key unavailable".to_owned())
    }

    fn profile_key_metadata(&self) -> Option<profile::ProfileKeyMetadata> {
        profile::production_profile_key_metadata(&self.lifecycle)
    }
}

#[cfg(target_os = "macos")]
fn configure_macos_panel(panel: &WebviewWindow) -> tauri::Result<()> {
    use objc2_app_kit::{NSWindow, NSWindowCollectionBehavior};

    panel.with_webview(|webview| unsafe {
        let window: &NSWindow = &*webview.ns_window().cast();
        let behavior = window.collectionBehavior()
            | NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary
            | NSWindowCollectionBehavior::Transient
            | NSWindowCollectionBehavior::IgnoresCycle;
        window.setCollectionBehavior(behavior);
    })?;

    Ok(())
}

#[cfg(target_os = "macos")]
fn configure_macos_window_for_current_space(window: &WebviewWindow) -> tauri::Result<()> {
    use objc2_app_kit::{NSWindow, NSWindowCollectionBehavior};

    window.with_webview(|webview| unsafe {
        let window: &NSWindow = &*webview.ns_window().cast();
        let behavior = window.collectionBehavior() | NSWindowCollectionBehavior::MoveToActiveSpace;
        window.setCollectionBehavior(behavior);
    })?;

    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Frame {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

fn panel_origin(tray: Frame, panel: PhysicalSize<u32>, monitor: Frame) -> PhysicalPosition<i32> {
    let desired_x = tray.x + (tray.width - f64::from(panel.width)) / 2.0;
    let desired_y = tray.y + tray.height + 6.0;
    let inset = 8.0;
    let min_x = monitor.x + inset;
    let max_x = monitor.x + monitor.width - f64::from(panel.width) - inset;
    let min_y = monitor.y + inset;
    let max_y = monitor.y + monitor.height - f64::from(panel.height) - inset;

    PhysicalPosition::new(
        desired_x.clamp(min_x, max_x).round() as i32,
        desired_y.clamp(min_y, max_y).round() as i32,
    )
}

fn physical_position(position: Position, scale_factor: f64) -> PhysicalPosition<f64> {
    match position {
        Position::Physical(position) => {
            PhysicalPosition::new(f64::from(position.x), f64::from(position.y))
        }
        Position::Logical(position) => position.to_physical(scale_factor),
    }
}

fn physical_size(size: Size, scale_factor: f64) -> PhysicalSize<f64> {
    match size {
        Size::Physical(size) => PhysicalSize::new(f64::from(size.width), f64::from(size.height)),
        Size::Logical(size) => size.to_physical(scale_factor),
    }
}

fn frame_for_rect(rect: Rect, scale_factor: f64) -> Frame {
    let position = physical_position(rect.position, scale_factor);
    let size = physical_size(rect.size, scale_factor);
    Frame {
        x: position.x,
        y: position.y,
        width: size.width,
        height: size.height,
    }
}

fn monitor_for_tray(window: &WebviewWindow, tray: Frame) -> tauri::Result<Frame> {
    let tray_center_x = tray.x + tray.width / 2.0;
    let tray_center_y = tray.y + tray.height / 2.0;
    let monitors = window.available_monitors()?;

    let monitor = monitors
        .iter()
        .find(|monitor| {
            let position = monitor.position();
            let size = monitor.size();
            let left = f64::from(position.x);
            let top = f64::from(position.y);
            tray_center_x >= left
                && tray_center_x < left + f64::from(size.width)
                && tray_center_y >= top
                && tray_center_y < top + f64::from(size.height)
        })
        .or_else(|| monitors.first())
        .ok_or_else(|| tauri::Error::AssetNotFound("monitor".into()))?;

    Ok(Frame {
        x: f64::from(monitor.position().x),
        y: f64::from(monitor.position().y),
        width: f64::from(monitor.size().width),
        height: f64::from(monitor.size().height),
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TrayForegroundDestination {
    Onboarding,
    Panel,
}

fn tray_foreground_destination(bootstrap_required: bool) -> TrayForegroundDestination {
    if bootstrap_required {
        TrayForegroundDestination::Onboarding
    } else {
        TrayForegroundDestination::Panel
    }
}

fn toggle_panel(app: &AppHandle, tray_rect: Rect) -> tauri::Result<()> {
    let destination = app
        .try_state::<DesktopLifecycle>()
        .map_or(TrayForegroundDestination::Panel, |lifecycle| {
            tray_foreground_destination(lifecycle.should_show_bootstrap())
        });
    match destination {
        TrayForegroundDestination::Onboarding => return show_onboarding(app),
        TrayForegroundDestination::Panel => {}
    }

    let Some(panel) = app.get_webview_window(PANEL_LABEL) else {
        return Ok(());
    };

    if panel.is_visible()? {
        panel.hide()?;
        return Ok(());
    }

    let scale_factor = panel.scale_factor()?;
    let tray = frame_for_rect(tray_rect, scale_factor);
    let monitor = monitor_for_tray(&panel, tray)?;
    let origin = panel_origin(tray, panel.outer_size()?, monitor);
    panel.set_position(origin)?;
    panel.show()?;
    panel.set_focus()?;
    if let Some(core) = app.try_state::<NativeCore>() {
        let _ = core.request_refresh(RefreshSource::StalePanelOpen);
    }
    Ok(())
}

#[tauri::command]
fn hide_panel(window: WebviewWindow, app: AppHandle) -> Result<(), String> {
    require_panel(&window)?;
    if let Some(panel) = app.get_webview_window(PANEL_LABEL) {
        panel.hide().map_err(|_| "panel unavailable".to_owned())?;
    }
    Ok(())
}

fn show_onboarding(app: &AppHandle) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(ONBOARDING_LABEL) {
        window.show()?;
        window.set_focus()?;
    }
    Ok(())
}

fn show_settings(app: &AppHandle, section: SettingsSection) -> tauri::Result<()> {
    if let Some(lifecycle) = app.try_state::<DesktopLifecycle>() {
        lifecycle.request_settings_section(section);
    }
    if let Some(window) = app.get_webview_window(SETTINGS_LABEL) {
        window.show()?;
        window.set_focus()?;
        window.emit(
            SETTINGS_NAVIGATION_EVENT,
            SettingsNavigationRequest { section },
        )?;
    }
    Ok(())
}

#[tauri::command]
fn open_settings(window: WebviewWindow, app: AppHandle) -> Result<(), String> {
    require_panel(&window)?;
    show_settings(&app, SettingsSection::General).map_err(|_| "settings unavailable".to_owned())
}

fn require_panel(window: &WebviewWindow) -> Result<(), String> {
    (window.label() == PANEL_LABEL)
        .then_some(())
        .ok_or_else(|| "command unavailable for this window".to_owned())
}

fn bounded_panel_height(height: f64) -> Result<f64, &'static str> {
    if !height.is_finite() || height <= 0.0 {
        return Err("invalid panel height");
    }

    Ok(height.ceil().clamp(MIN_PANEL_HEIGHT, MAX_PANEL_HEIGHT))
}

fn should_show_bootstrap_on_start(bootstrap_required: bool, launched_in_background: bool) -> bool {
    bootstrap_required && !launched_in_background
}

#[tauri::command]
fn resize_panel(window: WebviewWindow, height: f64) -> Result<(), String> {
    require_panel(&window)?;
    let bounded_height = bounded_panel_height(height).map_err(str::to_owned)?;
    window
        .set_size(LogicalSize::new(PANEL_WIDTH, bounded_height))
        .map_err(|_| "panel unavailable".to_owned())
}

#[tauri::command]
fn get_sanitized_state(
    window: WebviewWindow,
    core: State<'_, NativeCore>,
) -> Result<SanitizedDesktopStateV2, String> {
    require_panel(&window)?;
    core.panel_state().map_err(str::to_owned)
}

#[tauri::command]
fn request_refresh(
    window: WebviewWindow,
    core: State<'_, NativeCore>,
) -> Result<RefreshReceipt, String> {
    require_panel(&window)?;
    core.request_refresh(RefreshSource::Manual)
        .map_err(str::to_owned)
}

fn request_native_refresh(app: &AppHandle) -> Result<(), String> {
    app.state::<NativeCore>()
        .request_refresh(RefreshSource::Manual)
        .map_err(str::to_owned)?;
    Ok(())
}

fn require_settings(window: &WebviewWindow) -> Result<(), String> {
    (window.label() == SETTINGS_LABEL)
        .then_some(())
        .ok_or_else(|| "command unavailable for this window".to_owned())
}

fn require_onboarding(window: &WebviewWindow) -> Result<(), String> {
    (window.label() == ONBOARDING_LABEL)
        .then_some(())
        .ok_or_else(|| "command unavailable for this window".to_owned())
}

fn require_settings_or_onboarding(window: &WebviewWindow) -> Result<(), String> {
    matches!(window.label(), SETTINGS_LABEL | ONBOARDING_LABEL)
        .then_some(())
        .ok_or_else(|| "command unavailable for this window".to_owned())
}

fn require_profile_settings(
    window: &WebviewWindow,
    lifecycle: &DesktopLifecycle,
) -> Result<SettingsProfileAuthorization, String> {
    require_settings(window)?;
    lifecycle
        .authorize_profile_settings()
        .ok_or_else(|| "command unavailable for this section".to_owned())
}

#[tauri::command]
fn get_bootstrap_state(
    window: WebviewWindow,
    lifecycle: State<'_, DesktopLifecycle>,
) -> Result<BootstrapStateV2, String> {
    require_onboarding(&window)?;
    Ok(lifecycle.bootstrap_state())
}

#[tauri::command]
async fn complete_bootstrap(
    window: WebviewWindow,
    app: AppHandle,
    display_name: String,
) -> Result<BootstrapStateV2, String> {
    require_onboarding(&window)?;
    let lifecycle = app.state::<DesktopLifecycle>().inner().clone();
    let current = lifecycle.bootstrap_state();
    if lifecycle.bootstrap_completion_ready() {
        return Ok(current);
    }
    if current.profile_provisioning != lifecycle::ProfileProvisioningStatus::Ready {
        lifecycle
            .complete_bootstrap(&display_name)
            .map_err(str::to_owned)?;
        app.state::<NativeCore>()
            .set_profile_outcome(SanitizedProfileOutcome::ProfilePending)
            .map_err(str::to_owned)?;
    }
    let runtime = app.state::<ProfileRuntime>().inner().clone();
    let _ = tauri::async_runtime::spawn_blocking(move || runtime.attempt_now()).await;
    let state = lifecycle.bootstrap_state();
    if state.profile_provisioning == lifecycle::ProfileProvisioningStatus::Ready
        && !lifecycle.bootstrap_completion_ready()
    {
        return Err("Recovery Key pending".to_owned());
    }
    Ok(state)
}

fn launch_at_login_state(app: &AppHandle) -> LaunchAtLoginState {
    app.autolaunch()
        .is_enabled()
        .map(|enabled| LaunchAtLoginState::Available { enabled })
        .unwrap_or(LaunchAtLoginState::Unavailable)
}

fn settings_state_with_profile_key_metadata(
    lifecycle: &DesktopLifecycle,
    launch_at_login: LaunchAtLoginState,
    profile_runtime: &ProfileRuntime,
) -> SettingsStateV2 {
    let mut state = lifecycle.settings_state(launch_at_login);
    if state.profile_provisioning == lifecycle::ProfileProvisioningStatus::Ready {
        if let Some(metadata) = profile_runtime.profile_key_metadata() {
            state.profile_key_id = Some(metadata.id);
            state.recovery_key_suffix = Some(metadata.recovery_key_suffix);
        }
    }
    state
}

#[tauri::command]
fn get_settings_state(
    window: WebviewWindow,
    app: AppHandle,
    lifecycle: State<'_, DesktopLifecycle>,
    profile_runtime: State<'_, ProfileRuntime>,
) -> Result<SettingsStateV2, String> {
    require_settings(&window)?;
    Ok(settings_state_with_profile_key_metadata(
        &lifecycle,
        launch_at_login_state(&app),
        &profile_runtime,
    ))
}

#[tauri::command]
fn set_launch_at_login(
    window: WebviewWindow,
    app: AppHandle,
    lifecycle: State<'_, DesktopLifecycle>,
    profile_runtime: State<'_, ProfileRuntime>,
    enabled: bool,
) -> Result<SettingsStateV2, String> {
    require_settings(&window)?;
    let result = if enabled {
        app.autolaunch().enable()
    } else {
        app.autolaunch().disable()
    };
    let launch_at_login = if result.is_ok() {
        launch_at_login_state(&app)
    } else {
        LaunchAtLoginState::Unavailable
    };
    Ok(settings_state_with_profile_key_metadata(
        &lifecycle,
        launch_at_login,
        &profile_runtime,
    ))
}

#[tauri::command]
fn select_settings_section(
    window: WebviewWindow,
    lifecycle: State<'_, DesktopLifecycle>,
    section: SettingsSection,
) -> Result<(), String> {
    require_settings(&window)?;
    lifecycle.request_settings_section(section);
    Ok(())
}

#[tauri::command]
async fn reveal_recovery_key(
    window: WebviewWindow,
    lifecycle: State<'_, DesktopLifecycle>,
    profile_runtime: State<'_, ProfileRuntime>,
) -> Result<String, String> {
    let authorization = require_profile_settings(&window, &lifecycle)?;
    let runtime = profile_runtime.inner().clone();
    tauri::async_runtime::spawn_blocking(move || runtime.reveal_recovery_key(authorization))
        .await
        .map_err(|_| "Recovery Key unavailable".to_owned())?
}

#[tauri::command]
fn hide_surface(window: WebviewWindow) -> Result<(), String> {
    require_settings_or_onboarding(&window)?;
    window.hide().map_err(|_| "window unavailable".to_owned())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let process_started_at = Instant::now();
    let launched_in_background = env::args_os().any(|argument| argument == "--background");
    let mut app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(
            |app, arguments, _working_directory| {
                let background_request =
                    arguments.iter().any(|argument| argument == "--background");
                if !background_request
                    && app
                        .try_state::<DesktopLifecycle>()
                        .is_some_and(|lifecycle| lifecycle.should_show_bootstrap())
                {
                    let _ = show_onboarding(app);
                }
            },
        ))
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec!["--background"]),
        ))
        .invoke_handler(tauri::generate_handler![
            complete_bootstrap,
            get_bootstrap_state,
            get_sanitized_state,
            get_settings_state,
            hide_surface,
            hide_panel,
            open_settings,
            request_refresh,
            resize_panel,
            reveal_recovery_key,
            select_settings_section,
            set_launch_at_login
        ])
        .setup(move |app| {
            let database_path = app
                .path()
                .app_data_dir()
                .ok()
                .map(|directory| directory.join("touchgrassbar.sqlite3"));
            let lifecycle = database_path
                .as_deref()
                .and_then(|path| DesktopLifecycle::open(path).ok())
                .unwrap_or_else(DesktopLifecycle::unavailable);
            let core = database_path
                .as_deref()
                .and_then(|path| NativeCore::open(path).ok())
                .unwrap_or_else(NativeCore::unavailable);
            let show_bootstrap = should_show_bootstrap_on_start(
                lifecycle.should_show_bootstrap(),
                launched_in_background,
            );
            app.manage(lifecycle.clone());
            app.manage(core.clone());

            app.state::<NativeCore>()
                .set_profile_outcome(lifecycle.sanitized_profile_outcome())
                .map_err(std::io::Error::other)?;

            #[cfg(debug_assertions)]
            let development_instance = dev_instance::DevelopmentInstance::from_environment();
            #[cfg(debug_assertions)]
            if let Some(instance) = development_instance.as_ref() {
                for (label, title) in [
                    (PANEL_LABEL, "TouchGrassBar"),
                    (SETTINGS_LABEL, "TouchGrassBar Settings"),
                    (ONBOARDING_LABEL, "Welcome to TouchGrassBar"),
                ] {
                    if let Some(window) = app.get_webview_window(label) {
                        window.set_title(&instance.window_title(title))?;
                    }
                }
            }
            let profile_runtime = ProfileRuntime::start(lifecycle, app.handle().clone())?;
            profile_runtime.trigger();
            app.manage(profile_runtime);

            if let Some(panel) = app.get_webview_window(PANEL_LABEL) {
                panel.set_visible_on_all_workspaces(true)?;
                panel.set_always_on_top(true)?;
                #[cfg(target_os = "macos")]
                configure_macos_panel(&panel)?;
            }
            #[cfg(target_os = "macos")]
            for label in [SETTINGS_LABEL, ONBOARDING_LABEL] {
                if let Some(window) = app.get_webview_window(label) {
                    configure_macos_window_for_current_space(&window)?;
                }
            }

            let revision_notices = core.revision_notices().map_err(std::io::Error::other)?;
            let revision_notice_app = app.handle().clone();
            std::thread::Builder::new()
                .name("sanitized-state-revision-notices".to_owned())
                .spawn(move || {
                    while let Ok(notice) = revision_notices.recv() {
                        let _ = revision_notice_app
                            .emit::<RevisionNotice>(REVISION_NOTICE_EVENT, notice);
                    }
                })?;
            let refresh = MenuItemBuilder::with_id("refresh", "Refresh").build(app)?;
            let settings = MenuItemBuilder::with_id("settings", "Settings…").build(app)?;
            let profile = MenuItemBuilder::with_id("profile", "Profile & Recovery…").build(app)?;
            #[cfg(debug_assertions)]
            let quit_label = development_instance.as_ref().map_or_else(
                || "Quit TouchGrassBar".to_owned(),
                dev_instance::DevelopmentInstance::quit_label,
            );
            #[cfg(not(debug_assertions))]
            let quit_label = "Quit TouchGrassBar";
            let quit = MenuItemBuilder::with_id("quit", quit_label).build(app)?;
            let separator = PredefinedMenuItem::separator(app)?;
            let menu = MenuBuilder::new(app)
                .items(&[&refresh, &settings, &profile, &separator, &quit])
                .build()?;

            let tray_icon = tauri::image::Image::from_bytes(MENU_BAR_ICON)?;
            #[cfg(debug_assertions)]
            let tray_tooltip = development_instance.as_ref().map_or_else(
                || "TouchGrassBar".to_owned(),
                dev_instance::DevelopmentInstance::tooltip,
            );
            #[cfg(not(debug_assertions))]
            let tray_tooltip = "TouchGrassBar";
            let tray_builder = TrayIconBuilder::with_id("touchgrassbar")
                .tooltip(tray_tooltip)
                .icon(tray_icon)
                .icon_as_template(true)
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "refresh" => {
                        let _ = request_native_refresh(app);
                    }
                    "settings" => {
                        let _ = show_settings(app, SettingsSection::General);
                    }
                    "profile" => {
                        let _ = show_settings(app, SettingsSection::Profile);
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        rect,
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let toggle_started_at = Instant::now();
                        if toggle_panel(tray.app_handle(), rect).is_ok() {
                            eprintln!(
                                "touchgrassbar_metric panel_toggle_ms={}",
                                toggle_started_at.elapsed().as_millis()
                            );
                        }
                    }
                });
            #[cfg(debug_assertions)]
            let tray_builder = match development_instance.as_ref() {
                Some(instance) => tray_builder.title(instance.tag()),
                None => tray_builder,
            };
            tray_builder.build(app)?;

            if show_bootstrap {
                show_onboarding(app.handle())?;
            }

            eprintln!(
                "touchgrassbar_metric native_setup_ms={}",
                process_started_at.elapsed().as_millis()
            );

            Ok(())
        })
        .on_window_event(|window, event| match event {
            tauri::WindowEvent::Focused(true) => {
                if let Some(profile_runtime) = window.app_handle().try_state::<ProfileRuntime>() {
                    profile_runtime.trigger();
                }
            }
            tauri::WindowEvent::Focused(false) if window.label() == PANEL_LABEL => {
                let _ = window.hide();
            }
            tauri::WindowEvent::CloseRequested { api, .. }
                if matches!(window.label(), SETTINGS_LABEL | ONBOARDING_LABEL) =>
            {
                api.prevent_close();
                let _ = window.hide();
            }
            _ => {}
        })
        .build(tauri::generate_context!())
        .expect("failed to build TouchGrassBar");

    #[cfg(target_os = "macos")]
    {
        app.set_activation_policy(ActivationPolicy::Accessory);
        app.set_dock_visibility(false);
    }

    app.run(|app, event| match event {
        RunEvent::Resumed => {
            if let Some(core) = app.try_state::<NativeCore>() {
                let _ = core.request_refresh(RefreshSource::Wake);
            }
        }
        RunEvent::Exit => {
            if let Some(core) = app.try_state::<NativeCore>() {
                core.shutdown();
            }
        }
        _ => {}
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn native_config() -> serde_json::Value {
        serde_json::from_str(include_str!("../tauri.conf.json"))
            .expect("native configuration should be valid JSON")
    }

    fn panel_config(config: &serde_json::Value) -> &serde_json::Value {
        config
            .pointer("/app/windows")
            .and_then(serde_json::Value::as_array)
            .and_then(|windows| {
                windows.iter().find(|window| {
                    window.get("label").and_then(serde_json::Value::as_str) == Some(PANEL_LABEL)
                })
            })
            .expect("panel window should be configured")
    }

    #[test]
    fn production_csp_allows_bundled_provider_marks() {
        let config = native_config();
        let csp = config
            .pointer("/app/security/csp")
            .and_then(serde_json::Value::as_str)
            .expect("production CSP should be configured");
        let image_sources = csp
            .split(';')
            .find_map(|directive| {
                let mut tokens = directive.split_whitespace();
                (tokens.next() == Some("img-src")).then(|| tokens.collect::<Vec<_>>())
            })
            .expect("production CSP should configure image sources");

        assert!(
            image_sources.contains(&"data:"),
            "Vite-inlined provider marks require data: images"
        );
    }

    #[test]
    fn transparent_panel_does_not_stack_a_native_shadow() {
        let config = native_config();
        let panel = panel_config(&config);

        assert_eq!(
            panel.get("shadow").and_then(serde_json::Value::as_bool),
            Some(false),
            "the clipped CSS panel owns the rounded edge and shadow treatment"
        );
    }

    #[test]
    fn macos_bundle_launches_as_a_ui_element() {
        let config = native_config();
        assert_eq!(
            config
                .pointer("/bundle/macOS/infoPlist")
                .and_then(serde_json::Value::as_str),
            Some("Info.plist")
        );

        let info_plist = include_str!("../Info.plist");
        assert!(info_plist.contains("<key>LSUIElement</key>"));
        assert!(info_plist.contains("<true/>"));
        assert!(!info_plist.contains("LSBackgroundOnly"));
    }

    #[test]
    fn production_webviews_disable_local_file_drop() {
        let config = native_config();
        let windows = config
            .pointer("/app/windows")
            .and_then(serde_json::Value::as_array)
            .expect("native windows should be configured");

        assert!(windows.iter().all(|window| {
            window
                .get("dragDropEnabled")
                .and_then(serde_json::Value::as_bool)
                == Some(false)
        }));
    }

    #[test]
    fn webview_capability_excludes_broad_core_and_path_permissions() {
        let capability: serde_json::Value =
            serde_json::from_str(include_str!("../capabilities/default.json"))
                .expect("desktop capability should be valid JSON");
        let permissions = capability
            .get("permissions")
            .and_then(serde_json::Value::as_array)
            .expect("desktop permissions should be configured");
        let permission_names = permissions
            .iter()
            .filter_map(serde_json::Value::as_str)
            .collect::<Vec<_>>();

        assert!(!permission_names.contains(&"core:default"));
        assert!(permission_names.contains(&"core:event:allow-listen"));
        assert!(permission_names.contains(&"core:event:allow-unlisten"));
        assert!(permission_names.iter().all(|name| !name.contains("path")));
        assert!(permission_names.iter().all(|name| !name.contains("image")));
    }

    #[test]
    fn bootstrap_opens_only_for_an_incomplete_manual_launch() {
        assert!(should_show_bootstrap_on_start(true, false));
        assert!(!should_show_bootstrap_on_start(true, true));
        assert!(!should_show_bootstrap_on_start(false, false));
        assert!(!should_show_bootstrap_on_start(false, true));
    }

    #[test]
    fn only_incomplete_onboarding_routes_the_tray_to_onboarding() {
        assert_eq!(
            tray_foreground_destination(true),
            TrayForegroundDestination::Onboarding,
        );
        assert_eq!(
            tray_foreground_destination(false),
            TrayForegroundDestination::Panel,
        );
    }

    #[test]
    fn single_instance_plugin_is_registered_before_other_plugins() {
        let source = include_str!("lib.rs");
        let single_instance = source
            .find(".plugin(tauri_plugin_single_instance::init")
            .expect("single-instance plugin should be registered");

        for plugin in [
            ".plugin(tauri_plugin_process::init())",
            ".plugin(tauri_plugin_updater::Builder::new().build())",
            ".plugin(tauri_plugin_autostart::init",
        ] {
            assert!(
                single_instance < source.find(plugin).expect("plugin should be registered"),
                "single-instance must be the first plugin"
            );
        }
    }

    #[test]
    fn centers_panel_below_tray_icon() {
        let origin = panel_origin(
            Frame {
                x: 900.0,
                y: 0.0,
                width: 24.0,
                height: 24.0,
            },
            PhysicalSize::new(402, 640),
            Frame {
                x: 0.0,
                y: 0.0,
                width: 1728.0,
                height: 1117.0,
            },
        );
        assert_eq!(origin, PhysicalPosition::new(711, 30));
    }

    #[test]
    fn supports_monitors_with_negative_coordinates() {
        let origin = panel_origin(
            Frame {
                x: -1300.0,
                y: -900.0,
                width: 24.0,
                height: 24.0,
            },
            PhysicalSize::new(402, 640),
            Frame {
                x: -1728.0,
                y: -900.0,
                width: 1728.0,
                height: 1117.0,
            },
        );
        assert_eq!(origin, PhysicalPosition::new(-1489, -870));
    }

    #[test]
    fn clamps_panel_inside_monitor_edges() {
        let origin = panel_origin(
            Frame {
                x: 1700.0,
                y: 0.0,
                width: 24.0,
                height: 24.0,
            },
            PhysicalSize::new(402, 640),
            Frame {
                x: 0.0,
                y: 0.0,
                width: 1728.0,
                height: 1117.0,
            },
        );
        assert_eq!(origin, PhysicalPosition::new(1318, 30));
    }

    #[test]
    fn clamps_rendered_panel_height_to_safe_native_bounds() {
        assert_eq!(bounded_panel_height(689.2), Ok(690.0));
        assert_eq!(bounded_panel_height(200.0), Ok(MIN_PANEL_HEIGHT));
        assert_eq!(bounded_panel_height(900.0), Ok(MAX_PANEL_HEIGHT));
        assert_eq!(bounded_panel_height(f64::NAN), Err("invalid panel height"));
        assert_eq!(bounded_panel_height(0.0), Err("invalid panel height"));
    }
}
