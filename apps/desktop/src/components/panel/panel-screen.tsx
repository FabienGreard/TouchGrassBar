import { invoke } from "@tauri-apps/api/core";
import { useEffect, useSyncExternalStore } from "react";

import type { BrowserFixtureName } from "@/browserSanitizedDesktopStateAdapter";
import { PanelView } from "@/components/panel/panel-view";
import {
  currentDoomerboardPreviewRows,
  currentUsagePreview,
  friendsDoomerboardPreviewRows,
} from "@/previewFixtures";
import type { SanitizedDesktopStateDelivery } from "@/sanitizedDesktopStateDelivery";

type PanelScreenProps = {
  hasNativeRuntime: boolean;
  previewFixtureName: BrowserFixtureName;
  stateDelivery: SanitizedDesktopStateDelivery;
};

function PanelScreen({
  hasNativeRuntime,
  previewFixtureName,
  stateDelivery,
}: PanelScreenProps) {
  const deliveryView = useSyncExternalStore(
    stateDelivery.subscribe,
    stateDelivery.getSnapshot,
    stateDelivery.getSnapshot,
  );
  const hasCurrentPreview =
    previewFixtureName === "current" || previewFixtureName === "update";

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (
        hasNativeRuntime &&
        (event.key === "Escape" ||
          (event.metaKey && event.key.toLowerCase() === "w"))
      )
        void invoke("hide_panel");
      if (hasNativeRuntime && event.metaKey && event.key === ",")
        void invoke("open_settings");
    };
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
      doomerboardPreviewRows={
        import.meta.env.DEV && !hasNativeRuntime && hasCurrentPreview
          ? currentDoomerboardPreviewRows
          : undefined
      }
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
      tokenmaxxerPreviewRows={
        import.meta.env.DEV && !hasNativeRuntime && hasCurrentPreview
          ? friendsDoomerboardPreviewRows
          : undefined
      }
      usagePreview={
        import.meta.env.DEV && !hasNativeRuntime && hasCurrentPreview
          ? currentUsagePreview
          : undefined
      }
      updateAvailable={previewFixtureName === "update"}
    />
  );
}

export { PanelScreen };
