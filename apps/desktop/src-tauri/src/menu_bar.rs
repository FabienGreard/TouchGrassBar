use crate::quota_headroom::{
    HeadroomCompleteness, HeadroomFreshness, OverallQuotaHeadroom, RevisionedOverallQuotaHeadroom,
};

const COMPACT_APP_BAR_MARK: &[u8] =
    include_bytes!("../../../../packages/ui/src/assets/brand/grass-glyph-white.png");
const CANVAS_WIDTH: u32 = 356;
const CANVAS_HEIGHT: u32 = 320;
const MARK_LEFT: u32 = 50;
const METER_LEFT: u32 = 12;
const METER_TOP: u32 = 264;
const METER_WIDTH: u32 = 332;
const METER_HEIGHT: u32 = 48;
const TRACK_ALPHA: u8 = 96;
const SEGMENT_BRIGHT_WIDTH: u32 = 12;
const SEGMENT_STRIDE: u32 = 20;
const NARROW_SEGMENT_BRIGHT_HALF_HEIGHT: u32 = 2;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum MenuBarVisibleState {
    MarkOnly,
    Meter {
        fill_width: u32,
        segmented: bool,
        rounded_percent: u8,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct MenuBarPresentation {
    pub(crate) revision: u64,
    pub(crate) visible: MenuBarVisibleState,
}

pub(crate) struct MenuBarDelivery {
    current: MenuBarPresentation,
}

impl MenuBarDelivery {
    pub(crate) fn install<E>(
        initial: MenuBarPresentation,
        mut replace: impl FnMut(&MenuBarVisibleState) -> Result<(), E>,
    ) -> Result<Self, E> {
        replace(&initial.visible)?;
        Ok(Self { current: initial })
    }

    pub(crate) fn accept<E>(
        &mut self,
        next: MenuBarPresentation,
        mut replace: impl FnMut(&MenuBarVisibleState) -> Result<(), E>,
    ) -> Result<bool, E> {
        if next.revision <= self.current.revision {
            return Ok(false);
        }
        if next.visible == self.current.visible {
            self.current.revision = next.revision;
            return Ok(false);
        }
        replace(&next.visible)?;
        self.current = next;
        Ok(true)
    }
}

impl From<RevisionedOverallQuotaHeadroom> for MenuBarPresentation {
    fn from(value: RevisionedOverallQuotaHeadroom) -> Self {
        Self {
            revision: value.revision,
            visible: MenuBarVisibleState::from_headroom(value.headroom),
        }
    }
}

impl MenuBarVisibleState {
    pub(crate) fn from_headroom(headroom: OverallQuotaHeadroom) -> Self {
        let OverallQuotaHeadroom::Calculated {
            remaining_percent,
            freshness,
            completeness,
        } = headroom
        else {
            return Self::MarkOnly;
        };
        let remaining_percent = remaining_percent.clamp(0.0, 100.0);
        Self::Meter {
            fill_width: ((remaining_percent / 100.0) * f64::from(METER_WIDTH)).round() as u32,
            segmented: freshness == HeadroomFreshness::Stale
                || completeness == HeadroomCompleteness::Incomplete,
            rounded_percent: remaining_percent.round() as u8,
        }
    }

    pub(crate) fn label(&self) -> String {
        match self {
            Self::MarkOnly => "TouchGrassBar".to_owned(),
            Self::Meter {
                rounded_percent, ..
            } => format!("TouchGrassBar — {rounded_percent}%"),
        }
    }

    pub(crate) fn rendered_icon(&self) -> tauri::Result<tauri::image::Image<'static>> {
        let mark = tauri::image::Image::from_bytes(COMPACT_APP_BAR_MARK)?;
        debug_assert_eq!(mark.width(), 256);
        debug_assert!(mark.height() < CANVAS_HEIGHT);
        let mut rgba = vec![0; (CANVAS_WIDTH * CANVAS_HEIGHT * 4) as usize];
        for y in 0..mark.height() {
            let source_start = (y * mark.width() * 4) as usize;
            let source_end = source_start + (mark.width() * 4) as usize;
            let destination_start = ((y * CANVAS_WIDTH + MARK_LEFT) * 4) as usize;
            let destination_end = destination_start + (mark.width() * 4) as usize;
            rgba[destination_start..destination_end]
                .copy_from_slice(&mark.rgba()[source_start..source_end]);
        }

        if let Self::Meter {
            fill_width,
            segmented,
            ..
        } = self
        {
            for y in METER_TOP..METER_TOP + METER_HEIGHT {
                for offset in 0..METER_WIDTH {
                    let alpha = meter_alpha(offset, y - METER_TOP, *fill_width, *segmented);
                    let pixel = ((y * CANVAS_WIDTH + METER_LEFT + offset) * 4) as usize;
                    rgba[pixel..pixel + 4].copy_from_slice(&[255, 255, 255, alpha]);
                }
            }
        }

        Ok(tauri::image::Image::new_owned(
            rgba,
            CANVAS_WIDTH,
            CANVAS_HEIGHT,
        ))
    }
}

pub(crate) fn apply_to_tray<R: tauri::Runtime>(
    tray: &tauri::tray::TrayIcon<R>,
    visible: &MenuBarVisibleState,
) -> Result<(), &'static str> {
    let image = visible
        .rendered_icon()
        .map_err(|_| "menu-bar icon unavailable")?;
    let icon = image.try_into().map_err(|_| "menu-bar icon unavailable")?;
    let label = visible.label();

    tray.with_inner_tray_icon(move |inner| -> Result<(), &'static str> {
        inner
            .set_icon_with_as_template(Some(icon), true)
            .map_err(|_| "menu-bar icon unavailable")?;
        inner
            .set_tooltip(Some(&label))
            .map_err(|_| "menu-bar label unavailable")?;

        #[cfg(target_os = "macos")]
        {
            use objc2::MainThreadMarker;
            use objc2_app_kit::NSAccessibility;
            use objc2_foundation::NSString;

            let marker = MainThreadMarker::new().ok_or("menu-bar label unavailable")?;
            let status_item = inner.ns_status_item().ok_or("menu-bar label unavailable")?;
            let button = status_item
                .button(marker)
                .ok_or("menu-bar label unavailable")?;
            let accessibility_label = NSString::from_str(&label);
            button.setAccessibilityLabel(Some(&accessibility_label));
        }

        Ok(())
    })
    .map_err(|_| "menu-bar item unavailable")?
}

fn meter_alpha(offset: u32, y_offset: u32, fill_width: u32, segmented: bool) -> u8 {
    if !pill_contains_pixel(METER_WIDTH, METER_HEIGHT, offset, y_offset) {
        return 0;
    }
    let fill_width = fill_width.min(METER_WIDTH);
    if !pill_contains_pixel(fill_width, METER_HEIGHT, offset, y_offset) {
        return TRACK_ALPHA;
    }
    if !segmented {
        return u8::MAX;
    }
    if fill_width <= SEGMENT_BRIGHT_WIDTH {
        let center = METER_HEIGHT / 2;
        return if y_offset >= center - NARROW_SEGMENT_BRIGHT_HALF_HEIGHT
            && y_offset < center + NARROW_SEGMENT_BRIGHT_HALF_HEIGHT
        {
            u8::MAX
        } else {
            TRACK_ALPHA
        };
    }
    let distance_from_end = fill_width - offset - 1;
    if distance_from_end % SEGMENT_STRIDE < SEGMENT_BRIGHT_WIDTH {
        u8::MAX
    } else {
        TRACK_ALPHA
    }
}

fn pill_contains_pixel(width: u32, height: u32, x: u32, y: u32) -> bool {
    if width == 0 || height == 0 || x >= width || y >= height {
        return false;
    }

    let width = f64::from(width);
    let height = f64::from(height);
    let pixel_x = f64::from(x) + 0.5;
    let pixel_y = f64::from(y) + 0.5;
    let radius_y = height / 2.0;

    if width >= height {
        let radius = radius_y;
        if pixel_x >= radius && pixel_x <= width - radius {
            return true;
        }
        let center_x = if pixel_x < radius {
            radius
        } else {
            width - radius
        };
        let distance_x = (pixel_x - center_x) / radius;
        let distance_y = (pixel_y - radius) / radius;
        return distance_x * distance_x + distance_y * distance_y <= 1.0;
    }

    let radius_x = width / 2.0;
    let distance_x = (pixel_x - radius_x) / radius_x;
    let distance_y = (pixel_y - radius_y) / radius_y;
    distance_x * distance_x + distance_y * distance_y <= 1.0
}

#[cfg(debug_assertions)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhysicalFixtureMode {
    Unavailable,
    CurrentZero,
    CurrentThirtyFour,
    CurrentOneHundred,
    SegmentedThirtyFour,
    Sequence,
}

#[cfg(debug_assertions)]
#[derive(Clone, Debug)]
pub(crate) struct PhysicalMenuBarFixture {
    mode: PhysicalFixtureMode,
    sequence_index: std::sync::Arc<std::sync::Mutex<usize>>,
}

#[cfg(debug_assertions)]
impl PhysicalMenuBarFixture {
    pub(crate) fn from_environment(
        development_instance: bool,
    ) -> Result<Option<Self>, &'static str> {
        let Some(value) = std::env::var_os("TOUCHGRASS_MENU_BAR_FIXTURE") else {
            return Ok(None);
        };
        let value = value.to_str().ok_or("invalid menu-bar fixture value")?;
        Self::from_configured_value(value, development_instance)
    }

    fn from_configured_value(
        value: &str,
        development_instance: bool,
    ) -> Result<Option<Self>, &'static str> {
        if !development_instance {
            return Err("menu-bar fixture requires a development instance");
        }
        Self::from_value(value)
            .map(Some)
            .ok_or("invalid menu-bar fixture value")
    }

    fn from_value(value: &str) -> Option<Self> {
        let mode = match value {
            "unavailable" => PhysicalFixtureMode::Unavailable,
            "current-0" => PhysicalFixtureMode::CurrentZero,
            "current-34" => PhysicalFixtureMode::CurrentThirtyFour,
            "current-100" => PhysicalFixtureMode::CurrentOneHundred,
            "segmented-34" => PhysicalFixtureMode::SegmentedThirtyFour,
            "sequence" => PhysicalFixtureMode::Sequence,
            _ => return None,
        };
        Some(Self {
            mode,
            sequence_index: std::sync::Arc::new(std::sync::Mutex::new(0)),
        })
    }

    pub(crate) fn visible(&self) -> MenuBarVisibleState {
        match self.mode {
            PhysicalFixtureMode::Unavailable => MenuBarVisibleState::MarkOnly,
            PhysicalFixtureMode::CurrentZero => physical_meter(
                0.0,
                HeadroomFreshness::Current,
                HeadroomCompleteness::Complete,
            ),
            PhysicalFixtureMode::CurrentThirtyFour => physical_meter(
                34.0,
                HeadroomFreshness::Current,
                HeadroomCompleteness::Complete,
            ),
            PhysicalFixtureMode::CurrentOneHundred => physical_meter(
                100.0,
                HeadroomFreshness::Current,
                HeadroomCompleteness::Complete,
            ),
            PhysicalFixtureMode::SegmentedThirtyFour => physical_meter(
                34.0,
                HeadroomFreshness::Stale,
                HeadroomCompleteness::Incomplete,
            ),
            PhysicalFixtureMode::Sequence => {
                let index = self
                    .sequence_index
                    .lock()
                    .map(|index| *index)
                    .unwrap_or_default();
                sequence_state(index)
            }
        }
    }

    pub(crate) fn advance(&self) -> Option<MenuBarVisibleState> {
        if self.mode != PhysicalFixtureMode::Sequence {
            return None;
        }
        let Ok(mut index) = self.sequence_index.lock() else {
            return None;
        };
        let previous = sequence_state(*index);
        *index = (*index + 1) % PHYSICAL_SEQUENCE_LENGTH;
        let next = sequence_state(*index);
        (next != previous).then_some(next)
    }
}

#[cfg(debug_assertions)]
const PHYSICAL_SEQUENCE_LENGTH: usize = 6;

#[cfg(debug_assertions)]
fn sequence_state(index: usize) -> MenuBarVisibleState {
    match index % PHYSICAL_SEQUENCE_LENGTH {
        0 | 5 => MenuBarVisibleState::MarkOnly,
        1 => physical_meter(
            8.0,
            HeadroomFreshness::Current,
            HeadroomCompleteness::Incomplete,
        ),
        2 => physical_meter(
            34.0,
            HeadroomFreshness::Current,
            HeadroomCompleteness::Complete,
        ),
        3 => physical_meter(
            34.0,
            HeadroomFreshness::Stale,
            HeadroomCompleteness::Complete,
        ),
        4 => physical_meter(
            0.0,
            HeadroomFreshness::Current,
            HeadroomCompleteness::Complete,
        ),
        _ => unreachable!(),
    }
}

#[cfg(debug_assertions)]
fn physical_meter(
    remaining_percent: f64,
    freshness: HeadroomFreshness,
    completeness: HeadroomCompleteness,
) -> MenuBarVisibleState {
    MenuBarVisibleState::from_headroom(OverallQuotaHeadroom::Calculated {
        remaining_percent,
        freshness,
        completeness,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quota_headroom::{HeadroomCompleteness, HeadroomFreshness};

    fn calculated(
        remaining_percent: f64,
        freshness: HeadroomFreshness,
        completeness: HeadroomCompleteness,
    ) -> OverallQuotaHeadroom {
        OverallQuotaHeadroom::Calculated {
            remaining_percent,
            freshness,
            completeness,
        }
    }

    fn alpha(image: &tauri::image::Image<'_>, x: u32, y: u32) -> u8 {
        image.rgba()[((y * image.width() + x) * 4 + 3) as usize]
    }

    #[test]
    fn labels_are_exact_and_round_only_the_final_percentage() {
        assert_eq!(
            MenuBarVisibleState::from_headroom(OverallQuotaHeadroom::Unavailable).label(),
            "TouchGrassBar"
        );
        assert_eq!(
            MenuBarVisibleState::from_headroom(calculated(
                34.49,
                HeadroomFreshness::Current,
                HeadroomCompleteness::Complete,
            ))
            .label(),
            "TouchGrassBar — 34%"
        );
        assert_eq!(
            MenuBarVisibleState::from_headroom(calculated(
                34.5,
                HeadroomFreshness::Current,
                HeadroomCompleteness::Complete,
            ))
            .label(),
            "TouchGrassBar — 35%"
        );
    }

    #[test]
    fn unavailable_uses_the_current_compact_mark_without_a_meter() {
        let base = tauri::image::Image::from_bytes(COMPACT_APP_BAR_MARK).unwrap();
        let rendered = MenuBarVisibleState::MarkOnly.rendered_icon().unwrap();

        assert_eq!(rendered.width(), CANVAS_WIDTH);
        assert_eq!(rendered.height(), CANVAS_HEIGHT);
        for y in 0..base.height() {
            let base_start = (y * base.width() * 4) as usize;
            let base_end = base_start + (base.width() * 4) as usize;
            let rendered_start = ((y * rendered.width() + MARK_LEFT) * 4) as usize;
            let rendered_end = rendered_start + (base.width() * 4) as usize;
            assert_eq!(
                &rendered.rgba()[rendered_start..rendered_end],
                &base.rgba()[base_start..base_end],
                "the current compact app-bar mark pixels must stay intact"
            );
        }
        for y in base.height()..CANVAS_HEIGHT {
            assert!(
                (0..CANVAS_WIDTH).all(|x| alpha(&rendered, x, y) == 0),
                "unavailable headroom must not draw a meter"
            );
        }
    }

    #[test]
    fn zero_keeps_the_track_and_one_hundred_fills_it() {
        let empty = MenuBarVisibleState::from_headroom(calculated(
            0.0,
            HeadroomFreshness::Current,
            HeadroomCompleteness::Complete,
        ))
        .rendered_icon()
        .unwrap();
        let full = MenuBarVisibleState::from_headroom(calculated(
            100.0,
            HeadroomFreshness::Current,
            HeadroomCompleteness::Complete,
        ))
        .rendered_icon()
        .unwrap();

        let middle_y = METER_TOP + METER_HEIGHT / 2;
        for x in METER_LEFT..METER_LEFT + METER_WIDTH {
            assert_eq!(alpha(&empty, x, middle_y), TRACK_ALPHA);
            assert_eq!(alpha(&full, x, middle_y), u8::MAX);
        }
    }

    #[test]
    fn meter_is_large_at_menu_bar_scale() {
        let logical_width = f64::from(METER_WIDTH) * 18.0 / f64::from(CANVAS_HEIGHT);
        let logical_height = f64::from(METER_HEIGHT) * 18.0 / f64::from(CANVAS_HEIGHT);

        assert!(logical_width >= 18.5);
        assert!(logical_height >= 2.5);
        assert_eq!(METER_WIDTH * 12, METER_HEIGHT * 83);
    }

    #[test]
    fn meter_track_and_fill_are_rounded_pills() {
        let empty = MenuBarVisibleState::from_headroom(calculated(
            0.0,
            HeadroomFreshness::Current,
            HeadroomCompleteness::Complete,
        ))
        .rendered_icon()
        .unwrap();
        let full = MenuBarVisibleState::from_headroom(calculated(
            100.0,
            HeadroomFreshness::Current,
            HeadroomCompleteness::Complete,
        ))
        .rendered_icon()
        .unwrap();
        let half = MenuBarVisibleState::from_headroom(calculated(
            50.0,
            HeadroomFreshness::Current,
            HeadroomCompleteness::Complete,
        ))
        .rendered_icon()
        .unwrap();
        let middle_y = METER_TOP + METER_HEIGHT / 2;
        let half_end = METER_LEFT + METER_WIDTH / 2 - 1;

        assert_eq!(alpha(&empty, METER_LEFT, METER_TOP), 0);
        assert_eq!(alpha(&full, METER_LEFT, METER_TOP), 0);
        assert_eq!(
            alpha(&full, METER_LEFT + METER_WIDTH - 1, METER_TOP),
            0
        );
        assert_eq!(alpha(&empty, METER_LEFT, middle_y), TRACK_ALPHA);
        assert_eq!(alpha(&full, METER_LEFT, middle_y), u8::MAX);
        assert_eq!(alpha(&half, half_end, METER_TOP), TRACK_ALPHA);
        assert_eq!(alpha(&half, half_end, middle_y), u8::MAX);
    }

    #[test]
    fn current_complete_is_continuous_and_stale_or_incomplete_is_segmented() {
        let current = MenuBarVisibleState::from_headroom(calculated(
            50.0,
            HeadroomFreshness::Current,
            HeadroomCompleteness::Complete,
        ));
        let stale = MenuBarVisibleState::from_headroom(calculated(
            50.0,
            HeadroomFreshness::Stale,
            HeadroomCompleteness::Complete,
        ));
        let incomplete = MenuBarVisibleState::from_headroom(calculated(
            50.0,
            HeadroomFreshness::Current,
            HeadroomCompleteness::Incomplete,
        ));

        let MenuBarVisibleState::Meter {
            fill_width: current_width,
            segmented: false,
            ..
        } = current
        else {
            panic!("current meter must be continuous");
        };
        let MenuBarVisibleState::Meter {
            fill_width: stale_width,
            segmented: true,
            ..
        } = stale
        else {
            panic!("stale meter must be segmented");
        };
        let MenuBarVisibleState::Meter {
            fill_width: incomplete_width,
            segmented: true,
            ..
        } = incomplete
        else {
            panic!("incomplete meter must be segmented");
        };
        assert_eq!(current_width, stale_width);
        assert_eq!(current_width, incomplete_width);
    }

    #[test]
    fn segmented_texture_has_bright_shape_and_non_color_gaps_with_a_truthful_end() {
        let state = MenuBarVisibleState::from_headroom(calculated(
            50.0,
            HeadroomFreshness::Stale,
            HeadroomCompleteness::Incomplete,
        ));
        let rendered = state.rendered_icon().unwrap();
        let fill_end = METER_LEFT + METER_WIDTH / 2;
        let row = (METER_LEFT..fill_end)
            .map(|x| alpha(&rendered, x, METER_TOP + METER_HEIGHT / 2))
            .collect::<Vec<_>>();

        assert!(row.contains(&u8::MAX));
        assert!(row.contains(&TRACK_ALPHA));
        let middle_y = METER_TOP + METER_HEIGHT / 2;
        assert_eq!(alpha(&rendered, fill_end - 1, middle_y), u8::MAX);
        assert_eq!(alpha(&rendered, fill_end, middle_y), TRACK_ALPHA);
    }

    #[test]
    fn segmented_eight_percent_keeps_a_visible_gap_and_truthful_end() {
        let state = MenuBarVisibleState::from_headroom(calculated(
            8.0,
            HeadroomFreshness::Current,
            HeadroomCompleteness::Incomplete,
        ));
        let rendered = state.rendered_icon().unwrap();
        let fill_width = ((8.0 / 100.0) * f64::from(METER_WIDTH)).round() as u32;
        let row = (METER_LEFT..METER_LEFT + fill_width)
            .map(|x| alpha(&rendered, x, METER_TOP + METER_HEIGHT / 2))
            .collect::<Vec<_>>();

        assert!(row.contains(&u8::MAX));
        assert!(row.contains(&TRACK_ALPHA));
        let middle_y = METER_TOP + METER_HEIGHT / 2;
        assert_eq!(
            alpha(&rendered, METER_LEFT + fill_width - 1, middle_y),
            u8::MAX
        );
    }

    #[test]
    fn every_narrow_nonzero_segmented_fill_has_a_geometric_gap() {
        for remaining_percent in [0.2, 1.0, 4.0] {
            let state = MenuBarVisibleState::from_headroom(calculated(
                remaining_percent,
                HeadroomFreshness::Stale,
                HeadroomCompleteness::Complete,
            ));
            let rendered = state.rendered_icon().unwrap();
            let fill_width = ((remaining_percent / 100.0) * f64::from(METER_WIDTH)).round() as u32;
            assert!(fill_width > 0);
            let fill = (METER_TOP..METER_TOP + METER_HEIGHT)
                .flat_map(|y| {
                    (METER_LEFT..METER_LEFT + fill_width)
                        .map(|x| alpha(&rendered, x, y))
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>();

            assert!(fill.contains(&u8::MAX));
            assert!(fill.contains(&TRACK_ALPHA));
            let middle_y = METER_TOP + METER_HEIGHT / 2;
            assert_eq!(
                alpha(&rendered, METER_LEFT + fill_width - 1, middle_y),
                u8::MAX
            );
        }
    }

    #[test]
    fn template_alpha_is_legible_on_light_dark_retina_and_increase_contrast_fixtures() {
        let state = MenuBarVisibleState::from_headroom(calculated(
            50.0,
            HeadroomFreshness::Current,
            HeadroomCompleteness::Complete,
        ));
        let rendered = state.rendered_icon().unwrap();
        let middle_y = METER_TOP + METER_HEIGHT / 2;
        let track_alpha =
            f64::from(alpha(&rendered, METER_LEFT + METER_WIDTH - 1, middle_y)) / 255.0;
        let fill_alpha = f64::from(alpha(&rendered, METER_LEFT, middle_y)) / 255.0;

        assert!(rendered.width() >= 18 * 2);
        assert!(rendered.height() >= 18 * 2);
        assert!(
            f64::from(METER_HEIGHT) * 36.0 / f64::from(rendered.height()) >= 2.0,
            "the meter must cover at least two pixels at Retina menu-bar scale"
        );
        for (background, template_tint) in [(1.0, 0.0), (0.0, 1.0)] {
            let track = template_tint * track_alpha + background * (1.0 - track_alpha);
            let fill = template_tint * fill_alpha + background * (1.0 - fill_alpha);
            assert!((fill - track).abs() >= 0.5);
        }
    }

    #[test]
    fn equal_visible_output_does_not_change_for_a_revision_only_difference() {
        let first = MenuBarVisibleState::from_headroom(calculated(
            34.0,
            HeadroomFreshness::Current,
            HeadroomCompleteness::Complete,
        ));
        let next = MenuBarVisibleState::from_headroom(calculated(
            34.01,
            HeadroomFreshness::Current,
            HeadroomCompleteness::Complete,
        ));

        assert_eq!(first, next);
    }

    #[test]
    fn delivery_installs_once_and_skips_unchanged_or_old_revisions() {
        let initial = MenuBarPresentation {
            revision: 1,
            visible: MenuBarVisibleState::from_headroom(calculated(
                34.0,
                HeadroomFreshness::Current,
                HeadroomCompleteness::Complete,
            )),
        };
        let mut installs = 0;
        let mut delivery = MenuBarDelivery::install(initial.clone(), |_| {
            installs += 1;
            Ok::<(), ()>(())
        })
        .unwrap();

        let mut unchanged = initial.clone();
        unchanged.revision = 2;
        assert!(
            !delivery
                .accept(unchanged, |_| {
                    installs += 1;
                    Ok::<(), ()>(())
                })
                .unwrap()
        );
        assert!(
            !delivery
                .accept(initial, |_| {
                    installs += 1;
                    Ok::<(), ()>(())
                })
                .unwrap()
        );

        let changed = MenuBarPresentation {
            revision: 3,
            visible: MenuBarVisibleState::from_headroom(calculated(
                34.0,
                HeadroomFreshness::Stale,
                HeadroomCompleteness::Complete,
            )),
        };
        assert!(
            delivery
                .accept(changed, |_| {
                    installs += 1;
                    Ok::<(), ()>(())
                })
                .unwrap()
        );
        assert_eq!(installs, 2);
    }

    #[cfg(debug_assertions)]
    #[test]
    fn physical_fixture_values_are_closed_and_the_sequence_is_deterministic() {
        assert_eq!(
            PhysicalMenuBarFixture::from_configured_value("sequence", false).unwrap_err(),
            "menu-bar fixture requires a development instance"
        );
        assert!(PhysicalMenuBarFixture::from_configured_value("unknown", true).is_err());
        assert!(PhysicalMenuBarFixture::from_value("unknown").is_none());
        for value in [
            "unavailable",
            "current-0",
            "current-34",
            "current-100",
            "segmented-34",
            "sequence",
        ] {
            assert!(PhysicalMenuBarFixture::from_value(value).is_some());
        }

        let fixture = PhysicalMenuBarFixture::from_value("sequence").unwrap();
        let mut labels = vec![fixture.visible().label()];
        for _ in 1..PHYSICAL_SEQUENCE_LENGTH {
            labels.push(fixture.advance().unwrap().label());
        }
        assert_eq!(
            labels,
            [
                "TouchGrassBar",
                "TouchGrassBar — 8%",
                "TouchGrassBar — 34%",
                "TouchGrassBar — 34%",
                "TouchGrassBar — 0%",
                "TouchGrassBar",
            ]
        );
        assert_eq!(fixture.advance(), None);
    }
}
