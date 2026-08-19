const laptopInviteQuery = "(min-width: 901px)";
const reducedMotionQuery = "(prefers-reduced-motion: reduce)";
const stoppedFloatClearance = 24;
const noticeShadowClearance = 8;

type NoticeTravel = {
  start: number;
  end: number;
};

function depthProgress(index: number, progress: number) {
  if (index < 5) return progress ** 1.25;
  if (index < 13) return progress;
  if (index === 13) return progress ** 0.9;
  return progress ** 0.8;
}

function smoothStep(progress: number) {
  return progress * progress * (3 - 2 * progress);
}

export function installInviteParallax(documentObject: Document, windowObject: Window) {
  const section = documentObject.querySelector<HTMLElement>(".d-recruit-stage.d-invite-variant-d");
  const noticeLayer = section?.querySelector<HTMLElement>(".d-invite-note-rain");
  if (!section || !noticeLayer) return () => undefined;

  const header = documentObject.querySelector<HTMLElement>(".d-menubar");
  const laptopInvite = windowObject.matchMedia(laptopInviteQuery);
  const reducedMotion = windowObject.matchMedia(reducedMotionQuery);
  const notices = Array.from(noticeLayer.querySelectorAll<HTMLElement>(":scope > span"));
  const noticeTravel = new Map<HTMLElement, NoticeTravel>();
  let animationFrame = 0;
  let limitsNeedMeasurement = true;
  let disposed = false;
  let viewportListenersInstalled = false;

  const measureNoticeTravel = () => {
    for (const notice of notices) {
      notice.dataset.parallaxStopped = "true";
      notice.style.setProperty("translate", "0 0");
    }

    const sectionBounds = section.getBoundingClientRect();
    for (const notice of notices) {
      if (windowObject.getComputedStyle(notice).display === "none") {
        noticeTravel.delete(notice);
        notice.style.removeProperty("--d-note-parallax-start");
        notice.style.removeProperty("--d-note-parallax-max");
        continue;
      }

      const noticeBounds = notice.getBoundingClientRect();
      const travel = {
        start: sectionBounds.top - noticeBounds.top,
        end: Math.max(0, sectionBounds.bottom - noticeBounds.bottom - noticeShadowClearance),
      };
      noticeTravel.set(notice, travel);
      notice.style.setProperty("--d-note-parallax-start", `${travel.start.toFixed(2)}px`);
      notice.style.setProperty("--d-note-parallax-max", `${travel.end.toFixed(2)}px`);
    }
    limitsNeedMeasurement = false;
  };

  const applyNoticeOffsets = (centerDelta: number, phaseDistance: number) => {
    const entering = centerDelta < 0;
    const phaseProgress = Math.min(1, Math.abs(centerDelta) / phaseDistance);

    notices.forEach((notice, index) => {
      const travel = noticeTravel.get(notice);
      if (travel === undefined) {
        notice.style.removeProperty("translate");
        delete notice.dataset.parallaxStopped;
        return;
      }

      const easedProgress = depthProgress(index, smoothStep(phaseProgress));
      const target = entering ? travel.start : travel.end;
      const offset = target * easedProgress;
      notice.style.setProperty("translate", `0 ${offset.toFixed(2)}px`);

      const distanceFromEdge = Math.abs(target - offset);
      if (phaseProgress > 0 && distanceFromEdge <= stoppedFloatClearance) {
        notice.dataset.parallaxStopped = "true";
      } else {
        delete notice.dataset.parallaxStopped;
      }
    });
  };

  const update = () => {
    animationFrame = 0;
    if (limitsNeedMeasurement) measureNoticeTravel();
    const bounds = section.getBoundingClientRect();
    const sectionCenter = bounds.top + bounds.height / 2;
    const viewportTop = Math.max(
      0,
      Math.min(windowObject.innerHeight, header?.getBoundingClientRect().bottom ?? 0),
    );
    const availableHeight = Math.max(windowObject.innerHeight - viewportTop, 1);
    const viewportCenter = viewportTop + availableHeight / 2;
    const phaseDistance = Math.max((bounds.height + availableHeight) / 2, 1);
    applyNoticeOffsets(viewportCenter - sectionCenter, phaseDistance);
  };

  const scheduleUpdate = () => {
    if (animationFrame) return;
    animationFrame = windowObject.requestAnimationFrame(update);
  };

  const handleResize = () => {
    limitsNeedMeasurement = true;
    scheduleUpdate();
  };

  const setViewportListeners = (enabled: boolean) => {
    if (enabled === viewportListenersInstalled) return;
    viewportListenersInstalled = enabled;
    if (enabled) {
      windowObject.addEventListener("resize", handleResize, { passive: true });
      windowObject.addEventListener("scroll", scheduleUpdate, { passive: true });
      return;
    }
    windowObject.removeEventListener("resize", handleResize);
    windowObject.removeEventListener("scroll", scheduleUpdate);
  };

  const clearNoticeParallax = () => {
    noticeTravel.clear();
    for (const notice of notices) {
      notice.style.removeProperty("translate");
      notice.style.removeProperty("--d-note-parallax-start");
      notice.style.removeProperty("--d-note-parallax-max");
      delete notice.dataset.parallaxStopped;
    }
  };

  const syncMotionPreference = () => {
    const enabled = laptopInvite.matches && !reducedMotion.matches;
    setViewportListeners(enabled);
    if (enabled) {
      limitsNeedMeasurement = true;
      scheduleUpdate();
      return;
    }
    if (animationFrame) {
      windowObject.cancelAnimationFrame(animationFrame);
      animationFrame = 0;
    }
    clearNoticeParallax();
  };

  laptopInvite.addEventListener("change", syncMotionPreference);
  reducedMotion.addEventListener("change", syncMotionPreference);
  void documentObject.fonts?.ready.then(() => {
    if (disposed || !laptopInvite.matches || reducedMotion.matches) return;
    limitsNeedMeasurement = true;
    scheduleUpdate();
  });
  syncMotionPreference();

  return () => {
    disposed = true;
    if (animationFrame) windowObject.cancelAnimationFrame(animationFrame);
    laptopInvite.removeEventListener("change", syncMotionPreference);
    reducedMotion.removeEventListener("change", syncMotionPreference);
    setViewportListeners(false);
    clearNoticeParallax();
  };
}
