use std::sync::mpsc;

use block2::RcBlock;
use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSAccessibility, NSAlert, NSAlertFirstButtonReturn, NSModalResponse, NSSecureTextField,
    NSTextField, NSView, NSWindow,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use tauri::WebviewWindow;

use crate::profile::Secret;

const SHEET_WIDTH: f64 = 360.0;

pub(crate) struct RecoveryCredentials {
    pub(crate) recovery_key: Secret,
    pub(crate) touch_grass_id: String,
}

pub(crate) async fn request(
    window: &WebviewWindow,
) -> Result<Option<RecoveryCredentials>, &'static str> {
    let parent = window
        .ns_window()
        .map_err(|_| "Profile recovery unavailable")? as usize;
    let (send, receive) = mpsc::sync_channel(1);

    window
        .run_on_main_thread(move || {
            let Some(mtm) = MainThreadMarker::new() else {
                let _ = send.send(Err("Profile recovery unavailable"));
                return;
            };
            let parent = unsafe { &*(parent as *const NSWindow) };
            let alert = NSAlert::new(mtm);
            alert.setMessageText(&NSString::from_str("Recover a Profile"));
            alert.setInformativeText(&NSString::from_str(
                "Enter the TouchGrass ID and Recovery Key from your other Mac. This Mac will become the Active Mac.",
            ));
            let recover_button = alert.addButtonWithTitle(&NSString::from_str("Recover Profile"));
            let cancel_button = alert.addButtonWithTitle(&NSString::from_str("Cancel"));

            let accessory = NSView::new(mtm);
            accessory.setFrame(rect(0.0, 0.0, SHEET_WIDTH, 104.0));

            let id_label = NSTextField::labelWithString(&NSString::from_str("TouchGrass ID"), mtm);
            id_label.setFrame(rect(0.0, 82.0, SHEET_WIDTH, 18.0));
            let id_field = NSTextField::textFieldWithString(&NSString::new(), mtm);
            id_field.setFrame(rect(0.0, 54.0, SHEET_WIDTH, 24.0));
            id_field.setPlaceholderString(Some(&NSString::from_str("TG-…")));
            id_field.setAccessibilityLabel(Some(&NSString::from_str("TouchGrass ID")));

            let key_label = NSTextField::labelWithString(&NSString::from_str("Recovery Key"), mtm);
            key_label.setFrame(rect(0.0, 28.0, SHEET_WIDTH, 18.0));
            let key_field = NSSecureTextField::initWithFrame(mtm.alloc(), rect(0.0, 0.0, SHEET_WIDTH, 24.0));
            key_field.setPlaceholderString(Some(&NSString::from_str("48-character Recovery Key")));
            key_field.setAccessibilityLabel(Some(&NSString::from_str("Recovery Key")));

            accessory.addSubview(&id_label);
            accessory.addSubview(&id_field);
            accessory.addSubview(&key_label);
            accessory.addSubview(&key_field);
            alert.setAccessoryView(Some(&accessory));
            alert.layout();

            unsafe {
                id_field.setNextKeyView(Some(&key_field));
                key_field.setNextKeyView(Some(&recover_button));
                cancel_button.setNextKeyView(Some(&id_field));
            }

            let alert_window = alert.window();
            alert_window.makeFirstResponder(Some(&id_field));
            let completion = RcBlock::new(move |response: NSModalResponse| {
                if response != NSAlertFirstButtonReturn {
                    let _ = send.send(Ok(None));
                    return;
                }
                let touch_grass_id = id_field.stringValue().to_string().trim().to_owned();
                let recovery_key = key_field.stringValue().to_string().trim().to_owned();
                let _ = send.send(Ok(Some(RecoveryCredentials {
                    recovery_key: Secret::new(recovery_key),
                    touch_grass_id,
                })));
            });
            alert.beginSheetModalForWindow_completionHandler(parent, Some(&completion));
        })
        .map_err(|_| "Profile recovery unavailable")?;

    tauri::async_runtime::spawn_blocking(move || receive.recv())
        .await
        .map_err(|_| "Profile recovery unavailable")?
        .map_err(|_| "Profile recovery unavailable")?
}

fn rect(x: f64, y: f64, width: f64, height: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(width, height))
}
