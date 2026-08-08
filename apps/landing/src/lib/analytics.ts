type AnalyticsEvent = "download clicked" | "outbound link clicked";

type AnalyticsProperties = {
  destination?: string;
  placement: string;
};

const posthogKey = "phc_ocvsy75kVuyKwh8rstDHb2Tzw6WwRN75H85MrJuzf27D";
const posthogHost = "https://eu.i.posthog.com";

let analyticsPromise: Promise<typeof import("posthog-js/dist/module")> | undefined;

function loadAnalytics() {
  if (!posthogKey) return undefined;
  analyticsPromise ??= import("posthog-js/dist/module").then((module) => {
    module.default.init(posthogKey, {
      api_host: posthogHost,
      autocapture: false,
      capture_exceptions: false,
      capture_pageleave: false,
      capture_pageview: false,
      cookieless_mode: "always",
      disable_session_recording: true,
      person_profiles: "never",
    });
    module.default.capture("$pageview", {
      page_type: document.body.dataset.page || "landing",
    });
    return module;
  });
  return analyticsPromise;
}

function capture(event: AnalyticsEvent, properties: AnalyticsProperties) {
  void loadAnalytics()?.then(({ default: posthog }) => {
    posthog.capture(event, properties);
  });
}

function analyticsTarget(target: EventTarget | null) {
  return target instanceof Element
    ? target.closest<HTMLElement>("[data-analytics-event]")
    : null;
}

function installAnalytics() {
  if (!posthogKey) return;

  document.addEventListener("click", (event) => {
    const target = analyticsTarget(event.target);
    if (!target) return;

    const analyticsEvent = target.dataset.analyticsEvent;
    if (analyticsEvent !== "download clicked" && analyticsEvent !== "outbound link clicked") return;

    const destination = target instanceof HTMLAnchorElement ? target.href : undefined;
    capture(analyticsEvent, {
      ...(destination ? { destination } : {}),
      placement: target.dataset.analyticsPlacement || "unknown",
    });
  });

  if (window.requestIdleCallback) {
    window.requestIdleCallback(() => void loadAnalytics(), { timeout: 5_000 });
  } else {
    window.setTimeout(() => void loadAnalytics(), 3_000);
  }
}

export { installAnalytics };
