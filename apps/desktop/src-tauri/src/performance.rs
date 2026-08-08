use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use schemars::{JsonSchema, Schema, schema_for};
use serde::Serialize;

pub const PANEL_PAINT_REQUEST_EVENT: &str = "panel-paint-requested";

#[derive(Clone, Copy, Debug, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PanelPaintRequest {
    pub sequence: u64,
}

#[derive(Clone, Copy, Debug)]
struct PendingPanelPaint {
    request: PanelPaintRequest,
    source: PanelPaintSource,
    started_at: Instant,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PanelPaintSource {
    Synthetic,
    Tray,
}

impl PanelPaintSource {
    fn metric_name(self) -> &'static str {
        match self {
            Self::Synthetic => "synthetic",
            Self::Tray => "tray",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct CompletedPanelPaint {
    elapsed: Duration,
    source: PanelPaintSource,
}

#[derive(Default)]
struct PanelPaintProbeInner {
    next_sequence: AtomicU64,
    pending: Mutex<Option<PendingPanelPaint>>,
}

#[derive(Clone, Default)]
pub(crate) struct PanelPaintProbe {
    inner: Arc<PanelPaintProbeInner>,
}

impl PanelPaintProbe {
    pub(crate) fn begin(
        &self,
        started_at: Instant,
        source: PanelPaintSource,
    ) -> Result<PanelPaintRequest, ()> {
        let sequence = self
            .inner
            .next_sequence
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                current.checked_add(1)
            })
            .map_err(|_| ())?
            + 1;
        let request = PanelPaintRequest { sequence };
        *self.inner.pending.lock().map_err(|_| ())? = Some(PendingPanelPaint {
            request,
            source,
            started_at,
        });
        Ok(request)
    }

    pub(crate) fn pending(&self) -> Result<Option<PanelPaintRequest>, ()> {
        Ok(self
            .inner
            .pending
            .lock()
            .map_err(|_| ())?
            .as_ref()
            .map(|pending| pending.request))
    }

    fn acknowledge(&self, sequence: u64) -> Result<Option<CompletedPanelPaint>, ()> {
        let mut pending = self.inner.pending.lock().map_err(|_| ())?;
        let Some(sample) = pending.as_ref() else {
            return Ok(None);
        };
        if sample.request.sequence != sequence {
            return Ok(None);
        }
        let completed = CompletedPanelPaint {
            elapsed: sample.started_at.elapsed(),
            source: sample.source,
        };
        *pending = None;
        Ok(Some(completed))
    }

    pub(crate) fn cancel(&self) {
        if let Ok(mut pending) = self.inner.pending.lock() {
            *pending = None;
        }
    }

    fn cancel_sequence(&self, sequence: u64) {
        if let Ok(mut pending) = self.inner.pending.lock()
            && pending
                .as_ref()
                .is_some_and(|sample| sample.request.sequence == sequence)
        {
            *pending = None;
        }
    }
}

pub fn panel_paint_request_schema() -> Schema {
    schema_for!(PanelPaintRequest)
}

fn record_completed_panel_paint(completed: CompletedPanelPaint) {
    eprintln!(
        "touchgrassbar_metric panel_paint_source={} panel_paint_ms={:.3}",
        completed.source.metric_name(),
        completed.elapsed.as_secs_f64() * 1_000.0
    );
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn acknowledge_after_visible_frame(
    probe: PanelPaintProbe,
    sequence: u64,
) -> Result<(), ()> {
    if let Some(completed) = probe.acknowledge(sequence)? {
        record_completed_panel_paint(completed);
    }
    Ok(())
}

#[cfg(target_os = "macos")]
pub(crate) unsafe fn acknowledge_after_visible_frame(
    view: &objc2_app_kit::NSView,
    probe: PanelPaintProbe,
    sequence: u64,
) -> Result<(), ()> {
    use std::cell::{Cell, RefCell};

    use objc2::{
        DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, runtime::AnyObject,
        sel,
    };
    use objc2_foundation::{
        NSObject, NSObjectNSDelayedPerforming, NSObjectProtocol, NSRunLoop, NSRunLoopCommonModes,
    };
    use objc2_quartz_core::CADisplayLink;

    struct DisplayTargetIvars {
        display_link: RefCell<Option<objc2::rc::Retained<CADisplayLink>>>,
        probe: PanelPaintProbe,
        sequence: u64,
        ticks: Cell<u8>,
    }

    define_class!(
        // SAFETY:
        // - NSObject has no subclassing requirements.
        // - PanelPaintDisplayTarget does not implement Drop.
        #[unsafe(super = NSObject)]
        #[thread_kind = MainThreadOnly]
        #[ivars = DisplayTargetIvars]
        struct PanelPaintDisplayTarget;

        impl PanelPaintDisplayTarget {
            // SAFETY: CADisplayLink invokes this exact target-action signature.
            #[unsafe(method(panelPaintDisplayTick:))]
            fn display_tick(&self, _display_link: &CADisplayLink) {
                if self
                    .ivars()
                    .probe
                    .pending()
                    .ok()
                    .flatten()
                    .is_none_or(|request| request.sequence != self.ivars().sequence)
                {
                    self.finish();
                    return;
                }
                let ticks = self.ivars().ticks.get().saturating_add(1);
                self.ivars().ticks.set(ticks);
                if ticks < 2 {
                    return;
                }
                let completed = self
                    .ivars()
                    .probe
                    .acknowledge(self.ivars().sequence)
                    .ok()
                    .flatten();
                self.finish();
                if let Some(completed) = completed {
                    record_completed_panel_paint(completed);
                }
            }

            #[unsafe(method(panelPaintDisplayTimeout:))]
            fn display_timeout(&self, _argument: Option<&AnyObject>) {
                self.ivars().probe.cancel_sequence(self.ivars().sequence);
                self.finish();
            }
        }

        // SAFETY: NSObjectProtocol has no additional requirements.
        unsafe impl NSObjectProtocol for PanelPaintDisplayTarget {}
    );

    impl PanelPaintDisplayTarget {
        fn finish(&self) {
            if let Some(display_link) = self.ivars().display_link.borrow_mut().take() {
                display_link.invalidate();
            }
            // SAFETY: The target and selector are the exact delayed request
            // that this module schedules below.
            unsafe {
                NSObject::cancelPreviousPerformRequestsWithTarget_selector_object(
                    self,
                    sel!(panelPaintDisplayTimeout:),
                    None,
                );
            }
        }

        fn new(
            probe: PanelPaintProbe,
            sequence: u64,
            mtm: MainThreadMarker,
        ) -> objc2::rc::Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(DisplayTargetIvars {
                display_link: RefCell::new(None),
                probe,
                sequence,
                ticks: Cell::new(0),
            });
            // SAFETY: NSObject's initializer has this signature.
            unsafe { msg_send![super(this), init] }
        }
    }

    let mtm = MainThreadMarker::new().ok_or(())?;
    let target = PanelPaintDisplayTarget::new(probe, sequence, mtm);
    // SAFETY: The target method accepts one CADisplayLink argument and the
    // display link retains the target until it is invalidated.
    let display_link =
        unsafe { view.displayLinkWithTarget_selector(&target, sel!(panelPaintDisplayTick:)) };
    *target.ivars().display_link.borrow_mut() = Some(display_link.clone());
    // SAFETY: This function runs on the main thread through with_webview.
    unsafe {
        display_link.addToRunLoop_forMode(&NSRunLoop::mainRunLoop(), NSRunLoopCommonModes);
        target.performSelector_withObject_afterDelay(sel!(panelPaintDisplayTimeout:), None, 2.0);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completes_only_the_matching_pending_sample_once() {
        let probe = PanelPaintProbe::default();
        let request = probe
            .begin(Instant::now(), PanelPaintSource::Tray)
            .expect("start paint sample");

        assert_eq!(probe.pending().unwrap(), Some(request));
        assert_eq!(probe.acknowledge(request.sequence + 1).unwrap(), None);
        assert_eq!(
            probe
                .acknowledge(request.sequence)
                .unwrap()
                .expect("matching completion")
                .source,
            PanelPaintSource::Tray
        );
        assert_eq!(probe.acknowledge(request.sequence).unwrap(), None);
        assert_eq!(probe.pending().unwrap(), None);
    }

    #[test]
    fn newer_samples_replace_stale_work_and_hide_cancels_work() {
        let probe = PanelPaintProbe::default();
        let first = probe
            .begin(Instant::now(), PanelPaintSource::Tray)
            .expect("start first sample");
        let second = probe
            .begin(Instant::now(), PanelPaintSource::Synthetic)
            .expect("start second sample");

        assert!(second.sequence > first.sequence);
        assert_eq!(probe.acknowledge(first.sequence).unwrap(), None);
        assert_eq!(probe.pending().unwrap(), Some(second));

        probe.cancel();
        assert_eq!(probe.acknowledge(second.sequence).unwrap(), None);
        assert_eq!(probe.pending().unwrap(), None);
    }

    #[test]
    fn a_stale_timeout_does_not_cancel_a_newer_sample() {
        let probe = PanelPaintProbe::default();
        let first = probe
            .begin(Instant::now(), PanelPaintSource::Tray)
            .expect("start first sample");
        let second = probe
            .begin(Instant::now(), PanelPaintSource::Synthetic)
            .expect("start second sample");

        probe.cancel_sequence(first.sequence);
        assert_eq!(probe.pending().unwrap(), Some(second));
        probe.cancel_sequence(second.sequence);
        assert_eq!(probe.pending().unwrap(), None);
    }
}
