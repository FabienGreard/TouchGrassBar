import { invoke } from "@tauri-apps/api/core";
import { useEffect, useSyncExternalStore } from "react";

import { PanelView, type PanelViewProps } from "@/components/panel/panel-view";
import { createPanelKeyboardHandler } from "@/components/panel/panel-keyboard";
import type { SanitizedDesktopStateDelivery } from "@/native-state/sanitized-desktop-state-delivery";

type PanelPresentation = Pick<
  PanelViewProps,
  | "currentProfile"
  | "doomerboardRows"
  | "tokenmaxxerRows"
  | "updateAvailable"
  | "usagePresentation"
>;

type PanelScreenProps = {
  hasNativeRuntime: boolean;
  presentation?: PanelPresentation | undefined;
  stateDelivery: SanitizedDesktopStateDelivery;
};

function PanelScreen({
  hasNativeRuntime,
  presentation = {},
  stateDelivery,
}: PanelScreenProps) {
  const deliveryView = useSyncExternalStore(
    stateDelivery.subscribe,
    stateDelivery.getSnapshot,
    stateDelivery.getSnapshot,
  );
  useEffect(() => {
    const onKeyDown = createPanelKeyboardHandler({
      dispatch: (command) => void invoke(command),
      enabled: hasNativeRuntime,
    });
    window.addEventListener("keydown", onKeyDown);

    return () => {
      window.removeEventListener("keydown", onKeyDown);
    };
  }, [hasNativeRuntime]);

  useEffect(() => {
    if (!hasNativeRuntime || typeof ResizeObserver === "undefined") return;

    const panel = document.querySelector<HTMLElement>(
      '[data-slot="panel-shell"]',
    );
    if (!panel) return;

    let lastHeight = 0;
    const observer = new ResizeObserver(() => {
      const height = Math.ceil(panel.getBoundingClientRect().height);
      if (height === lastHeight) return;
      lastHeight = height;
      void invoke("resize_panel", { height });
    });
    observer.observe(panel);

    return () => observer.disconnect();
  }, [hasNativeRuntime]);

  return (
    <PanelView
      currentProfile={presentation.currentProfile}
      doomerboardRows={presentation.doomerboardRows}
      error={deliveryView.phase === "degraded"}
      nativeGlass
      onRefresh={() => {
        void stateDelivery.requestRefresh();
      }}
      onSettings={() => {
        if (hasNativeRuntime) void invoke("open_settings");
      }}
      onUpdate={() => undefined}
      refreshing={deliveryView.refreshing}
      state={deliveryView.snapshot}
      tokenmaxxerRows={presentation.tokenmaxxerRows}
      updateAvailable={presentation.updateAvailable}
      usagePresentation={presentation.usagePresentation}
    />
  );
}

export { PanelScreen };
export type { PanelPresentation, PanelScreenProps };
