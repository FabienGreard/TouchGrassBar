use std::sync::mpsc::SyncSender;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::NSObjectProtocol;
use objc2::{MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSBackingStoreType, NSBezelStyle, NSButton, NSColor, NSFont, NSModalResponse,
    NSModalResponseOK, NSTextField, NSWindow, NSWindowStyleMask, NSWindowTitleVisibility,
};
use objc2_foundation::{MainThreadMarker, NSObject, NSPoint, NSRect, NSSize, NSString};

use crate::profile::{RecoveryPresentation, RecoveryPresentationKind};

const INTRODUCTION: &str = "Use it with your TouchGrass ID to restore this Profile on another Mac.";
const VISUAL_ORDER: [&str; 6] = [
    "title",
    "introduction",
    "touch-grass-id",
    "recovery-key",
    "keychain-note",
    "action",
];

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RecoverySheetPalette {
    pub(crate) green: [f64; 4],
    pub(crate) ink: [f64; 4],
    pub(crate) ivory: [f64; 4],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RecoverySheetDesign {
    pub(crate) button_title: &'static str,
    pub(crate) introduction: &'static str,
    pub(crate) note: &'static str,
    pub(crate) palette: RecoverySheetPalette,
    pub(crate) size: (f64, f64),
    pub(crate) title: &'static str,
    pub(crate) uses_icon: bool,
    pub(crate) visual_order: [&'static str; 6],
}

pub(crate) fn design(kind: RecoveryPresentationKind) -> RecoverySheetDesign {
    let (title, button_title, note) = match kind {
        RecoveryPresentationKind::InitialDisclosure => (
            "Save your Recovery Key",
            "I saved a copy",
            "Stored in this Mac’s Keychain. Save a separate copy somewhere secure.",
        ),
        RecoveryPresentationKind::Reveal => {
            ("Recovery Key", "Done", "Stored in this Mac’s Keychain.")
        }
    };
    RecoverySheetDesign {
        button_title,
        introduction: INTRODUCTION,
        note,
        palette: RecoverySheetPalette {
            green: [0.098, 0.455, 0.239, 1.0],
            ink: [0.071, 0.071, 0.078, 1.0],
            ivory: [0.992, 0.984, 0.953, 1.0],
        },
        size: (440.0, 304.0),
        title,
        uses_icon: false,
        visual_order: VISUAL_ORDER,
    }
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and this class has no ivars.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = ()]
    struct RecoverySheetButtonTarget;

    // SAFETY: NSObjectProtocol has no additional requirements.
    unsafe impl NSObjectProtocol for RecoverySheetButtonTarget {}

    impl RecoverySheetButtonTarget {
        #[unsafe(method(confirmRecovery:))]
        fn confirm_recovery(&self, sender: &NSButton) {
            let Some(sheet) = sender.window() else {
                return;
            };
            let Some(parent) = sheet.sheetParent() else {
                return;
            };
            parent.endSheet_returnCode(&sheet, NSModalResponseOK);
        }
    }
);

impl RecoverySheetButtonTarget {
    fn new(main_thread: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(main_thread).set_ivars(());
        // SAFETY: NSObject init has the declared signature and initializes this subclass.
        unsafe { msg_send![super(this), init] }
    }
}

fn color([red, green, blue, alpha]: [f64; 4]) -> Retained<NSColor> {
    NSColor::colorWithSRGBRed_green_blue_alpha(red, green, blue, alpha)
}

fn label(
    text: &str,
    frame: NSRect,
    font: &NSFont,
    ink: &NSColor,
    main_thread: MainThreadMarker,
) -> Retained<NSTextField> {
    let label = NSTextField::wrappingLabelWithString(&NSString::from_str(text), main_thread);
    label.setFrame(frame);
    label.setFont(Some(font));
    label.setTextColor(Some(ink));
    label
}

pub(crate) fn begin(
    parent: &NSWindow,
    presentation: RecoveryPresentation,
    sender: SyncSender<bool>,
    main_thread: MainThreadMarker,
) {
    let design = design(presentation.kind);
    let ivory = color(design.palette.ivory);
    let ink = color(design.palette.ink);
    let green = color(design.palette.green);
    let title_font = NSFont::boldSystemFontOfSize(17.0);
    let body_font = NSFont::systemFontOfSize(12.0);
    let label_font = NSFont::boldSystemFontOfSize(11.0);
    let fixed_font =
        NSFont::userFixedPitchFontOfSize(12.0).unwrap_or_else(|| NSFont::systemFontOfSize(12.0));
    let style = NSWindowStyleMask::Titled
        | NSWindowStyleMask::DocModalWindow
        | NSWindowStyleMask::FullSizeContentView;
    let sheet = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(main_thread),
            NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(design.size.0, design.size.1),
            ),
            style,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    // SAFETY: This retained window outlives the sheet session and is released by Rust.
    unsafe { sheet.setReleasedWhenClosed(false) };
    sheet.setTitle(&NSString::from_str(design.title));
    sheet.setTitleVisibility(NSWindowTitleVisibility::Hidden);
    sheet.setTitlebarAppearsTransparent(true);
    sheet.setBackgroundColor(Some(&ivory));

    let content = sheet
        .contentView()
        .expect("Recovery sheet must have a content view");
    let title = label(
        design.title,
        NSRect::new(NSPoint::new(28.0, 254.0), NSSize::new(384.0, 24.0)),
        &title_font,
        &ink,
        main_thread,
    );
    let introduction = label(
        design.introduction,
        NSRect::new(NSPoint::new(28.0, 216.0), NSSize::new(384.0, 32.0)),
        &body_font,
        &ink,
        main_thread,
    );
    let touch_grass_id_label = label(
        "TouchGrass ID",
        NSRect::new(NSPoint::new(28.0, 188.0), NSSize::new(120.0, 16.0)),
        &label_font,
        &ink,
        main_thread,
    );
    let touch_grass_id = label(
        &presentation.touch_grass_id,
        NSRect::new(NSPoint::new(152.0, 185.0), NSSize::new(260.0, 20.0)),
        &fixed_font,
        &ink,
        main_thread,
    );
    let recovery_key_label = label(
        "Recovery Key",
        NSRect::new(NSPoint::new(28.0, 154.0), NSSize::new(120.0, 16.0)),
        &label_font,
        &ink,
        main_thread,
    );
    let recovery_key = NSTextField::textFieldWithString(
        &NSString::from_str(presentation.recovery_key.expose()),
        main_thread,
    );
    recovery_key.setFrame(NSRect::new(
        NSPoint::new(28.0, 112.0),
        NSSize::new(384.0, 32.0),
    ));
    recovery_key.setEditable(false);
    recovery_key.setSelectable(true);
    recovery_key.setBezeled(true);
    recovery_key.setDrawsBackground(true);
    recovery_key.setBackgroundColor(Some(&NSColor::whiteColor()));
    recovery_key.setFont(Some(&fixed_font));
    recovery_key.setTextColor(Some(&ink));
    let note = label(
        design.note,
        NSRect::new(NSPoint::new(28.0, 58.0), NSSize::new(384.0, 38.0)),
        &body_font,
        &ink,
        main_thread,
    );
    let target = RecoverySheetButtonTarget::new(main_thread);
    let button = unsafe {
        NSButton::buttonWithTitle_target_action(
            &NSString::from_str(design.button_title),
            Some(&target),
            Some(sel!(confirmRecovery:)),
            main_thread,
        )
    };
    button.setFrame(NSRect::new(
        NSPoint::new(276.0, 18.0),
        NSSize::new(136.0, 32.0),
    ));
    button.setBezelStyle(NSBezelStyle::Push);
    button.setBezelColor(Some(&green));
    button.setContentTintColor(Some(&ink));
    button.setFont(Some(&label_font));
    button.setKeyEquivalent(&NSString::from_str("\r"));

    content.addSubview(&title);
    content.addSubview(&introduction);
    content.addSubview(&touch_grass_id_label);
    content.addSubview(&touch_grass_id);
    content.addSubview(&recovery_key_label);
    content.addSubview(&recovery_key);
    content.addSubview(&note);
    content.addSubview(&button);

    let completion_target = target;
    let completion = RcBlock::new(move |response: NSModalResponse| {
        let _ = &completion_target;
        let _ = sender.send(response == NSModalResponseOK);
    });
    parent.beginSheet_completionHandler(&sheet, Some(&completion));
}
