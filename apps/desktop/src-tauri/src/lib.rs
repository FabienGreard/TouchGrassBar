mod sanitized;

use std::time::Instant;

use sanitized::SanitizedDesktopState;
use tauri::{
    ActivationPolicy, AppHandle, Manager, PhysicalPosition, PhysicalSize, Position, Rect, Size,
    WebviewWindow,
    menu::{MenuBuilder, MenuItemBuilder, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};
use tauri_plugin_autostart::MacosLauncher;
use tauri_plugin_autostart::ManagerExt as AutostartManagerExt;

const PANEL_LABEL: &str = "panel";

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

fn toggle_panel(app: &AppHandle, tray_rect: Rect) -> tauri::Result<()> {
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
    Ok(())
}

#[tauri::command]
fn hide_panel(app: AppHandle) -> tauri::Result<()> {
    if let Some(panel) = app.get_webview_window(PANEL_LABEL) {
        panel.hide()?;
    }
    Ok(())
}

#[tauri::command]
fn open_settings(app: AppHandle) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window("settings") {
        window.show()?;
        window.set_focus()?;
    }
    Ok(())
}

#[tauri::command]
fn get_sanitized_state() -> SanitizedDesktopState {
    sanitized::unavailable_state()
}

#[tauri::command]
fn launch_at_login_enabled(app: AppHandle) -> Result<bool, String> {
    app.autolaunch()
        .is_enabled()
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn set_launch_at_login(app: AppHandle, enabled: bool) -> Result<(), String> {
    if enabled {
        app.autolaunch().enable()
    } else {
        app.autolaunch().disable()
    }
    .map_err(|error| error.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let process_started_at = Instant::now();
    tauri::Builder::default()
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec!["--background"]),
        ))
        .invoke_handler(tauri::generate_handler![
            get_sanitized_state,
            hide_panel,
            launch_at_login_enabled,
            open_settings,
            set_launch_at_login
        ])
        .setup(move |app| {
            #[cfg(target_os = "macos")]
            {
                app.set_activation_policy(ActivationPolicy::Accessory);
                app.set_dock_visibility(false);
            }

            if let Some(panel) = app.get_webview_window(PANEL_LABEL) {
                panel.set_visible_on_all_workspaces(true)?;
                panel.set_always_on_top(true)?;
                #[cfg(target_os = "macos")]
                configure_macos_panel(&panel)?;
            }

            let refresh = MenuItemBuilder::with_id("refresh", "Refresh").build(app)?;
            let settings = MenuItemBuilder::with_id("settings", "Settings…").build(app)?;
            let quit = MenuItemBuilder::with_id("quit", "Quit TouchGrassBar").build(app)?;
            let separator = PredefinedMenuItem::separator(app)?;
            let menu = MenuBuilder::new(app)
                .items(&[&refresh, &settings, &separator, &quit])
                .build()?;

            TrayIconBuilder::with_id("touchgrassbar")
                .title("TG")
                .tooltip("TouchGrassBar")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "refresh" => {
                        // The background refresh coordinator will own this signal.
                    }
                    "settings" => {
                        let _ = open_settings(app.clone());
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
                })
                .build(app)?;

            eprintln!(
                "touchgrassbar_metric native_setup_ms={}",
                process_started_at.elapsed().as_millis()
            );

            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == PANEL_LABEL {
                if let tauri::WindowEvent::Focused(false) = event {
                    let _ = window.hide();
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("failed to run TouchGrassBar");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centers_panel_below_tray_icon() {
        let origin = panel_origin(
            Frame {
                x: 900.0,
                y: 0.0,
                width: 24.0,
                height: 24.0,
            },
            PhysicalSize::new(372, 640),
            Frame {
                x: 0.0,
                y: 0.0,
                width: 1728.0,
                height: 1117.0,
            },
        );
        assert_eq!(origin, PhysicalPosition::new(726, 30));
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
            PhysicalSize::new(372, 640),
            Frame {
                x: -1728.0,
                y: -900.0,
                width: 1728.0,
                height: 1117.0,
            },
        );
        assert_eq!(origin, PhysicalPosition::new(-1474, -870));
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
            PhysicalSize::new(372, 640),
            Frame {
                x: 0.0,
                y: 0.0,
                width: 1728.0,
                height: 1117.0,
            },
        );
        assert_eq!(origin, PhysicalPosition::new(1348, 30));
    }
}
