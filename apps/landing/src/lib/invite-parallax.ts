const desktopInviteQuery = "(min-width: 1181px)";
const reducedMotionQuery = "(prefers-reduced-motion: reduce)";

export function installInviteParallax(
  documentObject: Document,
  windowObject: Window,
) {
  const section = documentObject.querySelector<HTMLElement>(
    ".d-recruit-stage.d-invite-variant-d",
  );
  const noticeLayer = section?.querySelector<HTMLElement>(
    ".d-invite-note-rain",
  );
  if (!section || !noticeLayer) return () => undefined;

  const desktopInvite = windowObject.matchMedia(desktopInviteQuery);
  const reducedMotion = windowObject.matchMedia(reducedMotionQuery);
  let animationFrame = 0;

  const setDepthOffsets = (
    far: number,
    mid: number,
    near: number,
    landing: number,
  ) => {
    noticeLayer.style.setProperty("--d-invite-parallax-far", `${far.toFixed(2)}px`);
    noticeLayer.style.setProperty("--d-invite-parallax-mid", `${mid.toFixed(2)}px`);
    noticeLayer.style.setProperty("--d-invite-parallax-near", `${near.toFixed(2)}px`);
    noticeLayer.style.setProperty(
      "--d-invite-parallax-landing",
      `${landing.toFixed(2)}px`,
    );
  };

  const update = () => {
    animationFrame = 0;
    if (!desktopInvite.matches || reducedMotion.matches) {
      setDepthOffsets(0, 0, 0, 0);
      return;
    }

    const bounds = section.getBoundingClientRect();
    const travel = Math.max(bounds.height, windowObject.innerHeight) * 0.8;
    const startLead = windowObject.innerHeight * 0.22;
    const sectionCenter = bounds.top + bounds.height / 2;
    const viewportCenter = windowObject.innerHeight / 2;
    const progress = Math.max(
      0,
      Math.min(1, (viewportCenter - sectionCenter + startLead) / travel),
    );
    setDepthOffsets(
      progress * 72,
      progress * 144,
      progress * 216,
      progress * 52,
    );
  };

  const scheduleUpdate = () => {
    if (animationFrame) return;
    animationFrame = windowObject.requestAnimationFrame(update);
  };

  desktopInvite.addEventListener("change", scheduleUpdate);
  reducedMotion.addEventListener("change", scheduleUpdate);
  windowObject.addEventListener("resize", scheduleUpdate, { passive: true });
  windowObject.addEventListener("scroll", scheduleUpdate, { passive: true });
  scheduleUpdate();

  return () => {
    if (animationFrame) windowObject.cancelAnimationFrame(animationFrame);
    desktopInvite.removeEventListener("change", scheduleUpdate);
    reducedMotion.removeEventListener("change", scheduleUpdate);
    windowObject.removeEventListener("resize", scheduleUpdate);
    windowObject.removeEventListener("scroll", scheduleUpdate);
  };
}
