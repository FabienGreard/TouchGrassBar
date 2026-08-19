import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState, useSyncExternalStore } from "react";

import {
  createAddTokenmaxxerRequestGuard,
  type AddTokenmaxxerDialogStatus,
} from "@/components/dialogs/add-tokenmaxxer";
import { subscribeToPanelAddTokenmaxxer } from "@/components/panel/panel-add-tokenmaxxer";
import { createPanelKeyboardHandler } from "@/components/panel/panel-keyboard";
import { PanelView, type PanelViewProps } from "@/components/panel/panel-view";
import {
  createDoomerboardDelivery,
  defaultDoomerboardQuery,
} from "@/native-state/doomerboard-delivery";
import type { SanitizedDesktopStateDelivery } from "@/native-state/sanitized-desktop-state-delivery";
import { createTauriDoomerboardAdapter } from "@/native-state/tauri-doomerboard-adapter";
import { createTauriUpdateAdapter } from "@/native-state/tauri-update-adapter";
import { createUpdateDelivery } from "@/native-state/update-delivery";

type PanelPresentation = Pick<
  PanelViewProps,
  "currentProfile" | "doomerboardRows" | "tokenmaxxerRows" | "updateState" | "usagePresentation"
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

function PanelScreen({ hasNativeRuntime, presentation = {}, stateDelivery }: PanelScreenProps) {
  const [addTokenmaxxerOpen, setAddTokenmaxxerOpen] = useState(false);
  const [addTokenmaxxerStatus, setAddTokenmaxxerStatus] =
    useState<AddTokenmaxxerDialogStatus>("idle");
  const [addTokenmaxxerInFlight, setAddTokenmaxxerInFlight] = useState(false);
  const [addTokenmaxxerRequests] = useState(createAddTokenmaxxerRequestGuard);
  const [doomerboardSelection, setDoomerboardSelection] = useState(defaultDoomerboardQuery);
  const [doomerboard] = useState(() => createDoomerboardDelivery(createTauriDoomerboardAdapter()));
  const [updates] = useState(() => createUpdateDelivery(createTauriUpdateAdapter()));
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
    if (!hasNativeRuntime) return;
    void doomerboard.select(doomerboardSelection);
  }, [doomerboard, doomerboardSelection, hasNativeRuntime]);

  useEffect(() => {
    if (!hasNativeRuntime) return undefined;

    let active = true;
    let stop: (() => void) | undefined;
    void subscribeToPanelAddTokenmaxxer(() => {
      if (active) {
        addTokenmaxxerRequests.invalidate();
        setAddTokenmaxxerStatus("idle");
        setAddTokenmaxxerOpen(true);
      }
    }).then((stopListening) => {
      if (active) stop = stopListening;
      else stopListening();
    });

    return () => {
      active = false;
      stop?.();
    };
  }, [addTokenmaxxerRequests, hasNativeRuntime]);

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

    const panel = document.querySelector<HTMLElement>('[data-slot="panel-shell"]');
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
    presentation.currentProfile === undefined ? nativeProfile : presentation.currentProfile;
  const nativeDoomerboardRows =
    doomerboardView.view?.status === "ready"
      ? doomerboardView.view.rows.map((row) => {
          const presentedRow: NonNullable<PanelViewProps["doomerboardRows"]>[number] = {
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
  const updateActionsAvailable = hasNativeRuntime || presentation.updateState !== undefined;
  const updateState = presentation.updateState ?? updateView.state;
  const runUpdateAction = (action: () => Promise<boolean>) => {
    if (hasNativeRuntime) void action();
  };

  return (
    <PanelView
      addTokenmaxxerOpen={addTokenmaxxerOpen}
      addTokenmaxxerStatus={addTokenmaxxerInFlight ? "submitting" : addTokenmaxxerStatus}
      currentProfile={currentProfile}
      doomerboardRows={
        presentation.doomerboardRows ??
        (doomerboardSelection.audience === "global" ? nativeDoomerboardRows : undefined)
      }
      doomerboardSelection={doomerboardSelection}
      error={deliveryView.phase === "degraded"}
      nativeGlass
      onAddTokenmaxxer={(touchGrassId) => {
        const request = addTokenmaxxerRequests.begin();
        if (request === null) return;
        setAddTokenmaxxerInFlight(true);
        void (async () => {
          const outcome = hasNativeRuntime
            ? await doomerboard.addTokenmaxxer(touchGrassId)
            : ({ status: "unavailable" } as const);
          const current = addTokenmaxxerRequests.finish(request);
          setAddTokenmaxxerInFlight(addTokenmaxxerRequests.inFlight());
          if (!current) return;
          if (outcome.status === "added" || outcome.status === "already-added") {
            const nextSelection = { ...doomerboardSelection, audience: "mine" as const };
            setDoomerboardSelection(nextSelection);
            setAddTokenmaxxerOpen(false);
            setAddTokenmaxxerStatus("idle");
            void doomerboard.read(nextSelection);
            return;
          }
          setAddTokenmaxxerStatus(outcome.status);
        })();
      }}
      onAddTokenmaxxerInputChange={() => {
        addTokenmaxxerRequests.invalidate();
        setAddTokenmaxxerStatus("idle");
      }}
      onAddTokenmaxxerOpenChange={(open) => {
        addTokenmaxxerRequests.invalidate();
        setAddTokenmaxxerStatus("idle");
        setAddTokenmaxxerOpen(open);
      }}
      onDoomerboardSelectionChange={setDoomerboardSelection}
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
                updateState?.update.status === "failed" ? updates.retry : updates.install,
              );
            }
          : undefined
      }
      refreshing={deliveryView.refreshing}
      state={deliveryView.snapshot}
      tokenmaxxerRows={
        presentation.tokenmaxxerRows ??
        (doomerboardSelection.audience === "mine" ? nativeDoomerboardRows : undefined)
      }
      updateState={updateState}
      usagePresentation={presentation.usagePresentation}
    />
  );
}

export { PanelScreen };
export type { PanelPresentation, PanelScreenProps };
