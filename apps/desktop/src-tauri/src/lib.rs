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
    SettingsNavigationRequest, SettingsProfileAuthorization, SettingsSection, SettingsSelection,
    SettingsStateV2,
};
use profile::{
    RecoveryPresentation, RecoveryPresentationAudience, RecoveryPresentationKind,
    RecoverySheetPresenter,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RecoverySheetWindowState {
    focused: bool,
    label: &'static str,
    visible: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RecoverySheetAuthorization {
    parent_label: &'static str,
    settings_authorization: Option<SettingsProfileAuthorization>,
}

fn recovery_sheet_parent(
    windows: &[RecoverySheetWindowState],
    settings_selection: SettingsSelection,
    audience: RecoveryPresentationAudience,
) -> Option<RecoverySheetAuthorization> {
    let eligible = |window: &&RecoverySheetWindowState| match audience {
        RecoveryPresentationAudience::EligibleForeground => {
            window.label == ONBOARDING_LABEL
                || (window.label == SETTINGS_LABEL
                    && settings_selection.section == SettingsSection::Profile)
        }
        RecoveryPresentationAudience::ProfileSettings(_) => {
            window.label == SETTINGS_LABEL && settings_selection.section == SettingsSection::Profile
        }
    };
    windows
        .iter()
        .filter(eligible)
        .find(|window| window.focused)
        .or_else(|| {
            windows
                .iter()
                .filter(eligible)
                .find(|window| window.visible)
        })
        .map(|window| RecoverySheetAuthorization {
            parent_label: window.label,
            settings_authorization: match (window.label, audience) {
                (SETTINGS_LABEL, RecoveryPresentationAudience::ProfileSettings(authorization)) => {
                    Some(authorization)
                }
                (SETTINGS_LABEL, RecoveryPresentationAudience::EligibleForeground) => {
                    SettingsProfileAuthorization::from_selection(settings_selection)
                }
                _ => None,
            },
        })
}

fn recovery_sheet_authorization_is_current(
    authorization: RecoverySheetAuthorization,
    lifecycle: &DesktopLifecycle,
) -> bool {
    match authorization.parent_label {
        ONBOARDING_LABEL => authorization.settings_authorization.is_none(),
        SETTINGS_LABEL => authorization
            .settings_authorization
            .is_some_and(|selection| lifecycle.is_current_profile_settings(selection)),
        _ => false,
    }
}

#[cfg(target_os = "macos")]
struct NativeRecoverySheetPresenter {
    app: AppHandle,
    lifecycle: DesktopLifecycle,
}

#[cfg(target_os = "macos")]
impl RecoverySheetPresenter for NativeRecoverySheetPresenter {
    fn present(&self, presentation: RecoveryPresentation) -> bool {
        let windows = [ONBOARDING_LABEL, SETTINGS_LABEL, PANEL_LABEL]
            .into_iter()
            .filter_map(|label| self.app.get_webview_window(label))
            .collect::<Vec<_>>();
        let window_states = windows
            .iter()
            .filter_map(|window| {
                let label = match window.label() {
                    ONBOARDING_LABEL => ONBOARDING_LABEL,
                    SETTINGS_LABEL => SETTINGS_LABEL,
                    PANEL_LABEL => PANEL_LABEL,
                    _ => return None,
                };
                Some(RecoverySheetWindowState {
                    focused: window.is_focused().unwrap_or(false),
                    label,
                    visible: window.is_visible().unwrap_or(false),
                })
            })
            .collect::<Vec<_>>();
        let authorization = recovery_sheet_parent(
            &window_states,
            self.lifecycle.current_settings_selection(),
            presentation.audience,
        );
        let parent = authorization.and_then(|authorization| {
            windows
                .into_iter()
                .find(|window| window.label() == authorization.parent_label)
                .map(|window| (window, authorization))
        });
        let Some((parent, authorization)) = parent else {
            return false;
        };
        let (sender, receiver) = mpsc::sync_channel(1);
        let callback_sender = sender.clone();
        let lifecycle = self.lifecycle.clone();
        if parent
            .with_webview(move |webview| {
                use block2::RcBlock;
                use objc2::MainThreadMarker;
                use objc2_app_kit::{
                    NSAlert, NSApplication, NSColor, NSImage, NSModalResponse, NSWindow,
                };
                use objc2_foundation::{NSSize, NSString};

                if !recovery_sheet_authorization_is_current(authorization, &lifecycle) {
                    let _ = sender.send(false);
                    return;
                }

                let main_thread = MainThreadMarker::new()
                    .expect("native sheet callback must run on the main thread");
                let application = NSApplication::sharedApplication(main_thread);
                if !application.isActive() {
                    let _ = sender.send(false);
                    return;
                }
                let alert = NSAlert::new(main_thread);
                let icon_free_layout = NSImage::initWithSize(
                    main_thread.alloc(),
                    NSSize::new(0.0, 0.0),
                );
                // SAFETY: The non-nil image is retained by NSAlert and is valid for this sheet.
                unsafe { alert.setIcon(Some(&icon_free_layout)) };
                let (title, button) = match presentation.kind {
                    RecoveryPresentationKind::InitialDisclosure => {
                        ("Save your Recovery Key", "I saved my Recovery Key")
                    }
                    RecoveryPresentationKind::Reveal => ("Recovery Key", "Done"),
                };
                alert.setMessageText(&NSString::from_str(title));
                alert.setInformativeText(&NSString::from_str(&format!(
                    "TouchGrass ID\n{}\n\nRecovery Key\n{}\n\nStore this key in a safe place. TouchGrassBar cannot recover it for you.",
                    presentation.touch_grass_id,
                    presentation.recovery_key.expose()
                )));
                let saved = alert.addButtonWithTitle(&NSString::from_str(button));
                let ivory = NSColor::colorWithSRGBRed_green_blue_alpha(
                    0.992, 0.984, 0.953, 1.0,
                );
                let green = NSColor::colorWithSRGBRed_green_blue_alpha(
                    0.098, 0.455, 0.239, 1.0,
                );
                alert.window().setBackgroundColor(Some(&ivory));
                saved.setContentTintColor(Some(&green));
                let completion = RcBlock::new(move |_response: NSModalResponse| {
                    let _ = callback_sender.send(true);
                });
                let parent_window: &NSWindow = unsafe { &*webview.ns_window().cast() };
                alert.beginSheetModalForWindow_completionHandler(
                    parent_window,
                    Some(&completion),
                );
            })
            .is_err()
        {
            return false;
        }
        receiver.recv().unwrap_or(false)
    }
}

pub(crate) fn profile_attempt_metric<T, E>(attempt: &Result<T, E>) -> &'static str {
    match attempt {
        Ok(_) => "touchgrassbar_metric profile_attempt=complete",
        Err(_) => "touchgrassbar_metric profile_attempt=pending",
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProfileRetryRequest {
    Automatic,
    ProfileSettings(SettingsProfileAuthorization),
}

#[derive(Clone)]
struct ProfileRetryMailbox {
    pending: Arc<Mutex<Option<ProfileRetryRequest>>>,
    wake: mpsc::SyncSender<()>,
}

impl ProfileRetryMailbox {
    fn new() -> (Self, mpsc::Receiver<()>) {
        let (wake, receiver) = mpsc::sync_channel(1);
        (
            Self {
                pending: Arc::new(Mutex::new(None)),
                wake,
            },
            receiver,
        )
    }

    fn request(&self, request: ProfileRetryRequest) {
        let Ok(mut pending) = self.pending.lock() else {
            return;
        };
        *pending = Some(request);
        let _ = self.wake.try_send(());
    }

    fn take(&self) -> Option<ProfileRetryRequest> {
        self.pending.lock().ok()?.take()
    }
}

impl ProfileRetryRequest {
    fn audience(self) -> RecoveryPresentationAudience {
        match self {
            Self::Automatic => RecoveryPresentationAudience::EligibleForeground,
            Self::ProfileSettings(authorization) => {
                RecoveryPresentationAudience::ProfileSettings(authorization)
            }
        }
    }
}

#[derive(Clone)]
struct ProfileRuntime {
    coordinator: Arc<std::sync::Mutex<profile::ProfileCoordinator>>,
    lifecycle: DesktopLifecycle,
    retry: ProfileRetryMailbox,
}

impl ProfileRuntime {
    #[cfg(target_os = "macos")]
    fn start(lifecycle: DesktopLifecycle, app: AppHandle) -> std::io::Result<Self> {
        let runtime_lifecycle = lifecycle.clone();
        let presenter = Arc::new(NativeRecoverySheetPresenter {
            app: app.clone(),
            lifecycle: lifecycle.clone(),
        });
        let coordinator = Arc::new(std::sync::Mutex::new(profile::production_coordinator(
            lifecycle, presenter,
        )));
        let retry_coordinator = Arc::clone(&coordinator);
        let (retry, requests) = ProfileRetryMailbox::new();
        let worker_retry = retry.clone();
        std::thread::Builder::new()
            .name("profile-provisioning-retry".to_owned())
            .spawn(move || {
                loop {
                    let request = match requests.recv_timeout(Duration::from_secs(300)) {
                        Ok(()) => {
                            let Some(request) = worker_retry.take() else {
                                continue;
                            };
                            request
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => ProfileRetryRequest::Automatic,
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    };
                    let Ok(coordinator) = retry_coordinator.lock() else {
                        eprintln!("touchgrassbar_metric profile_attempt=pending");
                        continue;
                    };
                    let attempt = coordinator.retry_pending(request.audience());
                    if let Ok(Some(profile)) = &attempt {
                        let _ = app
                            .state::<NativeCore>()
                            .set_profile_outcome(profile.clone());
                    }
                    eprintln!("{}", profile_attempt_metric(&attempt));
                }
            })?;
        Ok(Self {
            coordinator,
            lifecycle: runtime_lifecycle,
            retry,
        })
    }

    fn trigger(&self) {
        self.retry.request(ProfileRetryRequest::Automatic);
    }

    fn trigger_from_profile_settings(&self, authorization: SettingsProfileAuthorization) {
        self.retry
            .request(ProfileRetryRequest::ProfileSettings(authorization));
    }

    fn reveal_recovery_key(
        &self,
        authorization: SettingsProfileAuthorization,
    ) -> Result<(), String> {
        let disclosure_was_pending = self.lifecycle.pending_recovery_disclosure();
        self.coordinator
            .lock()
            .map_err(|_| "Recovery Key unavailable".to_owned())?
            .reveal_recovery_key(disclosure_was_pending, authorization)
            .map_err(|_| "Recovery Key unavailable".to_owned())
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

fn tray_foreground_destination(
    bootstrap_required: bool,
    recovery_disclosure_pending: bool,
) -> TrayForegroundDestination {
    if bootstrap_required || recovery_disclosure_pending {
        TrayForegroundDestination::Onboarding
    } else {
        TrayForegroundDestination::Panel
    }
}

fn toggle_panel(app: &AppHandle, tray_rect: Rect) -> tauri::Result<()> {
    let destination =
        app.try_state::<DesktopLifecycle>()
            .map_or(TrayForegroundDestination::Panel, |lifecycle| {
                tray_foreground_destination(
                    lifecycle.should_show_bootstrap(),
                    lifecycle.pending_recovery_disclosure(),
                )
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
fn complete_bootstrap(
    window: WebviewWindow,
    lifecycle: State<'_, DesktopLifecycle>,
    profile_runtime: State<'_, ProfileRuntime>,
    core: State<'_, NativeCore>,
    display_name: String,
) -> Result<BootstrapStateV2, String> {
    require_onboarding(&window)?;
    let state = lifecycle
        .complete_bootstrap(&display_name)
        .map_err(str::to_owned)?;
    core.set_profile_outcome(SanitizedProfileOutcome::ProfilePending)
        .map_err(str::to_owned)?;
    profile_runtime.trigger();
    Ok(state)
}

fn launch_at_login_state(app: &AppHandle) -> LaunchAtLoginState {
    app.autolaunch()
        .is_enabled()
        .map(|enabled| LaunchAtLoginState::Available { enabled })
        .unwrap_or(LaunchAtLoginState::Unavailable)
}

#[tauri::command]
fn get_settings_state(
    window: WebviewWindow,
    app: AppHandle,
    lifecycle: State<'_, DesktopLifecycle>,
) -> Result<SettingsStateV2, String> {
    require_settings(&window)?;
    Ok(lifecycle.settings_state(launch_at_login_state(&app)))
}

#[tauri::command]
fn set_launch_at_login(
    window: WebviewWindow,
    app: AppHandle,
    lifecycle: State<'_, DesktopLifecycle>,
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
    Ok(lifecycle.settings_state(launch_at_login))
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
fn request_recovery_disclosure(
    window: WebviewWindow,
    lifecycle: State<'_, DesktopLifecycle>,
    profile_runtime: State<'_, ProfileRuntime>,
) -> Result<(), String> {
    let authorization = require_profile_settings(&window, &lifecycle)?;
    profile_runtime.trigger_from_profile_settings(authorization);
    Ok(())
}

#[tauri::command]
async fn reveal_recovery_key(
    window: WebviewWindow,
    lifecycle: State<'_, DesktopLifecycle>,
    profile_runtime: State<'_, ProfileRuntime>,
) -> Result<(), String> {
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
            request_recovery_disclosure,
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
    fn pending_recovery_disclosure_routes_the_next_tray_foreground_to_onboarding() {
        assert_eq!(
            tray_foreground_destination(false, true),
            TrayForegroundDestination::Onboarding,
        );
        assert_eq!(
            tray_foreground_destination(true, false),
            TrayForegroundDestination::Onboarding,
        );
        assert_eq!(
            tray_foreground_destination(false, false),
            TrayForegroundDestination::Panel,
        );
    }

    #[test]
    fn recovery_sheet_parent_is_limited_to_onboarding_and_profile_settings() {
        let profile_selection = SettingsSelection {
            section: SettingsSection::Profile,
            revision: 7,
        };
        let profile_authorization = SettingsProfileAuthorization::from_selection(profile_selection)
            .expect("Profile Settings authorization");
        let cases = [
            (
                "focused onboarding",
                SettingsSelection {
                    section: SettingsSection::General,
                    revision: 6,
                },
                vec![RecoverySheetWindowState {
                    focused: true,
                    label: ONBOARDING_LABEL,
                    visible: true,
                }],
                Some(RecoverySheetAuthorization {
                    parent_label: ONBOARDING_LABEL,
                    settings_authorization: None,
                }),
            ),
            (
                "focused Profile settings",
                profile_selection,
                vec![RecoverySheetWindowState {
                    focused: true,
                    label: SETTINGS_LABEL,
                    visible: true,
                }],
                Some(RecoverySheetAuthorization {
                    parent_label: SETTINGS_LABEL,
                    settings_authorization: Some(profile_authorization),
                }),
            ),
            (
                "visible Profile settings",
                profile_selection,
                vec![RecoverySheetWindowState {
                    focused: false,
                    label: SETTINGS_LABEL,
                    visible: true,
                }],
                Some(RecoverySheetAuthorization {
                    parent_label: SETTINGS_LABEL,
                    settings_authorization: Some(profile_authorization),
                }),
            ),
            (
                "General settings",
                SettingsSelection {
                    section: SettingsSection::General,
                    revision: 8,
                },
                vec![RecoverySheetWindowState {
                    focused: true,
                    label: SETTINGS_LABEL,
                    visible: true,
                }],
                None,
            ),
            (
                "compact panel",
                profile_selection,
                vec![RecoverySheetWindowState {
                    focused: true,
                    label: PANEL_LABEL,
                    visible: true,
                }],
                None,
            ),
        ];

        for (label, section, windows, expected) in cases {
            assert_eq!(
                recovery_sheet_parent(
                    &windows,
                    section,
                    RecoveryPresentationAudience::EligibleForeground,
                ),
                expected,
                "{label}"
            );
        }

        let lifecycle = DesktopLifecycle::unavailable();
        lifecycle.request_settings_section(SettingsSection::Profile);
        let profile_authorization = lifecycle
            .authorize_profile_settings()
            .expect("Profile Settings authorization");
        let authorization = RecoverySheetAuthorization {
            parent_label: SETTINGS_LABEL,
            settings_authorization: Some(profile_authorization),
        };
        assert!(recovery_sheet_authorization_is_current(
            authorization,
            &lifecycle,
        ));
        lifecycle.request_settings_section(SettingsSection::General);
        assert!(!recovery_sheet_authorization_is_current(
            authorization,
            &lifecycle,
        ));
        lifecycle.request_settings_section(SettingsSection::Profile);
        assert!(!recovery_sheet_authorization_is_current(
            authorization,
            &lifecycle,
        ));
    }

    #[test]
    fn profile_retry_mailbox_keeps_the_latest_settings_authorization() {
        let lifecycle = DesktopLifecycle::unavailable();
        lifecycle.request_settings_section(SettingsSection::Profile);
        let stale = lifecycle
            .authorize_profile_settings()
            .expect("initial Profile Settings authorization");
        lifecycle.request_settings_section(SettingsSection::General);
        lifecycle.request_settings_section(SettingsSection::Profile);
        let current = lifecycle
            .authorize_profile_settings()
            .expect("current Profile Settings authorization");

        let (mailbox, receiver) = ProfileRetryMailbox::new();
        mailbox.request(ProfileRetryRequest::ProfileSettings(stale));
        mailbox.request(ProfileRetryRequest::ProfileSettings(current));

        receiver.recv().expect("one retry wake");
        let ProfileRetryRequest::ProfileSettings(authorization) =
            mailbox.take().expect("latest retry request")
        else {
            panic!("Profile Settings retry expected");
        };
        assert!(lifecycle.is_current_profile_settings(authorization));
        assert!(mailbox.take().is_none());
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
