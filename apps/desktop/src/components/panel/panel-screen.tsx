import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState, useSyncExternalStore } from "react";

import { subscribeToPanelAddTokenmaxxer } from "@/components/panel/panel-add-tokenmaxxer";
import { createPanelKeyboardHandler } from "@/components/panel/panel-keyboard";
import { PanelView, type PanelViewProps } from "@/components/panel/panel-view";
import type { SanitizedDesktopStateDelivery } from "@/native-state/sanitized-desktop-state-delivery";
import { createTauriUpdateAdapter } from "@/native-state/tauri-update-adapter";
import { createUpdateDelivery } from "@/native-state/update-delivery";

type PanelPresentation = Pick<
  PanelViewProps,
  | "currentProfile"
  | "doomerboardRows"
  | "tokenmaxxerRows"
  | "updateState"
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
  const [addTokenmaxxerOpen, setAddTokenmaxxerOpen] = useState(false);
  const [updates] = useState(() =>
    createUpdateDelivery(createTauriUpdateAdapter()),
  );
  const deliveryView = useSyncExternalStore(
    stateDelivery.subscribe,
    stateDelivery.getSnapshot,
    stateDelivery.getSnapshot,
  );
  const updateView = useSyncExternalStore(
    updates.subscribe,
    updates.getSnapshot,
    updates.getSnapshot,
  );

  useEffect(() => {
    if (!hasNativeRuntime) return undefined;
    let disposed = false;
    let stop: () => void = () => undefined;
    void updates.activate().then((unsubscribe) => {
      if (disposed) unsubscribe();
      else stop = unsubscribe;
    });
    return () => {
      disposed = true;
      stop();
    };
  }, [hasNativeRuntime, updates]);

  useEffect(() => {
    if (!hasNativeRuntime) return undefined;

    let active = true;
    let stop: (() => void) | undefined;
    void subscribeToPanelAddTokenmaxxer(() => {
      if (active) setAddTokenmaxxerOpen(true);
    }).then((stopListening) => {
      if (active) stop = stopListening;
      else stopListening();
    });

    return () => {
      active = false;
      stop?.();
    };
  }, [hasNativeRuntime]);

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

  const nativeProfile =
    deliveryView.snapshot?.profile?.status === "ready"
      ? {
          id: `#${deliveryView.snapshot.profile.touchGrassId}`,
          name: deliveryView.snapshot.profile.displayName,
        }
      : null;
  const currentProfile =
    presentation.currentProfile === undefined
      ? nativeProfile
      : presentation.currentProfile;
  const updateActionsAvailable =
    hasNativeRuntime || presentation.updateState !== undefined;
  const runUpdateAction = (action: () => Promise<boolean>) => {
    if (hasNativeRuntime) void action();
  };

  return (
    <PanelView
      addTokenmaxxerOpen={addTokenmaxxerOpen}
      currentProfile={currentProfile}
      doomerboardRows={presentation.doomerboardRows}
      error={deliveryView.phase === "degraded"}
      nativeGlass
      onAddTokenmaxxerOpenChange={setAddTokenmaxxerOpen}
      onCheck={
        updateActionsAvailable
          ? () => {
              runUpdateAction(updates.check);
            }
          : undefined
      }
      onDefer={
        updateActionsAvailable
          ? () => {
              runUpdateAction(updates.defer);
            }
          : undefined
      }
      onInstall={
        updateActionsAvailable
          ? () => {
              runUpdateAction(updates.install);
            }
          : undefined
      }
      onOpenLatestDmg={
        updateActionsAvailable
          ? () => {
              runUpdateAction(updates.openLatestDmg);
            }
          : undefined
      }
      onRefresh={() => {
        void stateDelivery.requestRefresh();
      }}
      onSettings={() => {
        if (hasNativeRuntime) void invoke("open_settings");
      }}
      onRetry={
        updateActionsAvailable
          ? () => {
              runUpdateAction(updates.retry);
            }
          : undefined
      }
      refreshing={deliveryView.refreshing}
      state={deliveryView.snapshot}
      tokenmaxxerRows={presentation.tokenmaxxerRows}
      updateState={presentation.updateState ?? updateView.state}
      usagePresentation={presentation.usagePresentation}
    />
  );
}

export { PanelScreen };
export type { PanelPresentation, PanelScreenProps };
