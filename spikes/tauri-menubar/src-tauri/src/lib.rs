use tauri::{
    ActivationPolicy, AppHandle, Manager, PhysicalPosition, PhysicalSize, Position, Rect, Size,
    WebviewWindow,
    menu::{MenuBuilder, MenuItemBuilder, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};

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
        eprintln!(
            "spike: macOS panel collection behavior: {}",
            behavior.bits()
        );
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![hide_panel])
        .setup(|app| {
            let is_smoke_harness = app.config().identifier.ends_with(".smoke");
            #[cfg(target_os = "macos")]
            {
                if !is_smoke_harness {
                    app.set_activation_policy(ActivationPolicy::Accessory);
                    app.set_dock_visibility(false);
                }
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
                .tooltip("TouchGrassBar viability spike")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "refresh" => eprintln!("spike: refresh selected"),
                    "settings" => {
                        if let Some(window) = app.get_webview_window("settings") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
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
                        if let Err(error) = toggle_panel(tray.app_handle(), rect) {
                            eprintln!("spike: failed to toggle panel: {error}");
                        }
                    }
                })
                .build(app)?;

            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == PANEL_LABEL {
                if let tauri::WindowEvent::Focused(focused) = event {
                    eprintln!("spike: panel focus changed: {focused}");
                    if !focused {
                        let _ = window.hide();
                    }
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("failed to run TouchGrassBar menu-bar viability spike");
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
            PhysicalSize::new(360, 520),
            Frame {
                x: 0.0,
                y: 0.0,
                width: 1728.0,
                height: 1117.0,
            },
        );
        assert_eq!(origin, PhysicalPosition::new(732, 30));
    }

    #[test]
    fn supports_monitor_with_negative_coordinates() {
        let origin = panel_origin(
            Frame {
                x: -1300.0,
                y: -900.0,
                width: 24.0,
                height: 24.0,
            },
            PhysicalSize::new(360, 520),
            Frame {
                x: -1728.0,
                y: -900.0,
                width: 1728.0,
                height: 1117.0,
            },
        );
        assert_eq!(origin, PhysicalPosition::new(-1468, -870));
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
            PhysicalSize::new(360, 520),
            Frame {
                x: 0.0,
                y: 0.0,
                width: 1728.0,
                height: 1117.0,
            },
        );
        assert_eq!(origin, PhysicalPosition::new(1360, 30));
    }
}
