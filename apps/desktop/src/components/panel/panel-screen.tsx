import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState, useSyncExternalStore } from "react";

import { subscribeToPanelAddTokenmaxxer } from "@/components/panel/panel-add-tokenmaxxer";
import { createPanelKeyboardHandler } from "@/components/panel/panel-keyboard";
import { PanelView, type PanelViewProps } from "@/components/panel/panel-view";
import { createDoomerboardDelivery } from "@/native-state/doomerboard-delivery";
import type { SanitizedDesktopStateDelivery } from "@/native-state/sanitized-desktop-state-delivery";
import { createTauriDoomerboardAdapter } from "@/native-state/tauri-doomerboard-adapter";
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

const compactTokenScore = new Intl.NumberFormat("en-US", {
  maximumFractionDigits: 1,
  notation: "compact",
});

function PanelScreen({
  hasNativeRuntime,
  presentation = {},
  stateDelivery,
}: PanelScreenProps) {
  const [addTokenmaxxerOpen, setAddTokenmaxxerOpen] = useState(false);
  const [expanded, setExpanded] = useState(false);
  const [doomerboard] = useState(() =>
    createDoomerboardDelivery(createTauriDoomerboardAdapter()),
  );
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
  const doomerboardView = useSyncExternalStore(
    doomerboard.subscribe,
    doomerboard.getSnapshot,
    doomerboard.getSnapshot,
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
    let disposed = false;
    let stop: () => void = () => undefined;
    void doomerboard.activate().then((unsubscribe) => {
      if (disposed) unsubscribe();
      else stop = unsubscribe;
    });
    return () => {
      disposed = true;
      stop();
    };
  }, [doomerboard, hasNativeRuntime]);

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
          displayName: deliveryView.snapshot.profile.displayName,
          touchGrassId: `#${deliveryView.snapshot.profile.touchGrassId}`,
        }
      : null;
  const currentProfile =
    presentation.currentProfile === undefined
      ? nativeProfile
      : presentation.currentProfile;
  const nativeDoomerboardRows =
    doomerboardView.phase === "ready" &&
    doomerboardView.view?.status === "ready"
      ? doomerboardView.view.rows.map((row) => {
          const presentedRow: NonNullable<
            PanelViewProps["doomerboardRows"]
          >[number] = {
            displayName: row.displayName,
            rank: row.rank,
            tokenScore: compactTokenScore.format(row.tokenScore),
            touchGrassId: `#${row.touchGrassId}`,
          };
          if (
            deliveryView.snapshot?.profile?.status === "ready" &&
            deliveryView.snapshot.profile.touchGrassId === row.touchGrassId
          ) {
            presentedRow.note = "YOU";
          }
          return presentedRow;
        })
      : undefined;
  const updateActionsAvailable =
    hasNativeRuntime || presentation.updateState !== undefined;
  const updateState = presentation.updateState ?? updateView.state;
  const runUpdateAction = (action: () => Promise<boolean>) => {
    if (hasNativeRuntime) void action();
  };

  return (
    <PanelView
      addTokenmaxxerOpen={addTokenmaxxerOpen}
      currentProfile={currentProfile}
      doomerboardRows={
        presentation.doomerboardRows ?? nativeDoomerboardRows
      }
      error={deliveryView.phase === "degraded"}
      expanded={expanded}
      nativeGlass
      onAddTokenmaxxerOpenChange={setAddTokenmaxxerOpen}
      onExpandedChange={(nextExpanded) => {
        setExpanded(nextExpanded);
        if (!hasNativeRuntime) return;
        void invoke("set_panel_expanded", { expanded: nextExpanded }).catch(
          () =>
            setExpanded((current) =>
              current === nextExpanded ? !nextExpanded : current,
            ),
        );
      }}
      onRefresh={() => {
        void stateDelivery.requestRefresh();
      }}
      onSettings={() => {
        if (hasNativeRuntime) void invoke("open_settings");
      }}
      onUpdate={
        updateActionsAvailable
          ? () => {
              runUpdateAction(
                updateState?.update.status === "failed"
                  ? updates.retry
                  : updates.install,
              );
            }
          : undefined
      }
      refreshing={deliveryView.refreshing}
      state={deliveryView.snapshot}
      tokenmaxxerRows={presentation.tokenmaxxerRows}
      updateState={updateState}
      usagePresentation={presentation.usagePresentation}
    />
  );
}

export { PanelScreen };
export type { PanelPresentation, PanelScreenProps };
