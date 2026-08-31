import { invoke } from "@tauri-apps/api/core";
import { focusManager, type QueryClient, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef, useState, useSyncExternalStore } from "react";

import {
  type AddTokenmaxxerFailure,
  createAddTokenmaxxerRequestGuard,
} from "@/components/dialogs/add-tokenmaxxer";
import { subscribeToPanelAddTokenmaxxer } from "@/components/panel/panel-add-tokenmaxxer";
import { createPanelKeyboardHandler } from "@/components/panel/panel-keyboard";
import { PanelView, type PanelViewProps } from "@/components/panel/panel-view";
import {
  addTokenmaxxer,
  cancelDoomerboardAudience,
  cancelDoomerboardRankingDay,
  createDoomerboardQueryOptions,
  currentRankingDay,
  defaultDoomerboardQuery,
  doomerboardProfileAudienceFilter,
  doomerboardRankingDayKey,
  prefetchDoomerboardSelections,
  type DoomerboardPort,
  type DoomerboardPortOutcome,
  type DoomerboardQuery,
} from "@/native-state/doomerboard-query";
import type { SanitizedDesktopStateDelivery } from "@/native-state/sanitized-desktop-state-delivery";
import { createTauriDoomerboardAdapter } from "@/native-state/tauri-doomerboard-adapter";
import { createTauriUpdateAdapter } from "@/native-state/tauri-update-adapter";
import { createUpdateDelivery } from "@/native-state/update-delivery";

type PanelPresentation = Pick<
  PanelViewProps,
  | "currentProfile"
  | "doomerboardLoading"
  | "doomerboardRows"
  | "onUpdate"
  | "tokenmaxxerRows"
  | "updateState"
  | "usagePresentation"
>;

type PanelScreenProps = {
  doomerboardPort?: DoomerboardPort | undefined;
  hasNativeRuntime: boolean;
  presentation?: PanelPresentation | undefined;
  stateDelivery: SanitizedDesktopStateDelivery;
};

type DoomerboardCacheState = {
  controller: AbortController;
  prefetchStatus: "waiting" | "running" | "complete" | "canceled";
  profileKey: string;
  rankingDay: string;
};

type UseDoomerboardCacheInput = {
  activeSelection: DoomerboardQuery;
  client: QueryClient;
  dataReady: boolean;
  hasNativeRuntime: boolean;
  native: DoomerboardPort;
  profileKey: string | null;
  rankingDay: string;
  setRankingDay: (rankingDay: string) => void;
};

const compactTokenScore = new Intl.NumberFormat("en-US", {
  maximumFractionDigits: 1,
  notation: "compact",
});
const apiEquivalentCost = new Intl.NumberFormat("en-US", {
  currency: "USD",
  maximumFractionDigits: 2,
  minimumFractionDigits: 2,
  style: "currency",
});

function retainAsyncSubscription(start: () => Promise<DoomerboardPortOutcome<() => void>>) {
  let disposed = false;
  let stop: (() => void) | null = null;
  void start()
    .then((subscription) => {
      if (!subscription.ok) return;
      if (disposed) subscription.value();
      else stop = subscription.value;
    })
    .catch(() => undefined);
  return () => {
    disposed = true;
    stop?.();
  };
}

function deliveredProfileKey(stateDelivery: SanitizedDesktopStateDelivery) {
  const profile = stateDelivery.getSnapshot().snapshot?.profile;
  return profile?.status === "ready" ? profile.touchGrassId : null;
}

function useDoomerboardCache({
  activeSelection,
  client,
  dataReady,
  hasNativeRuntime,
  native,
  profileKey,
  rankingDay,
  setRankingDay,
}: UseDoomerboardCacheInput) {
  const cacheState = useRef<DoomerboardCacheState | null>(null);

  useEffect(() => {
    if (!hasNativeRuntime) return undefined;
    return retainAsyncSubscription(() =>
      native.subscribe(() => {
        const currentCacheState = cacheState.current;
        if (
          currentCacheState?.profileKey === profileKey &&
          currentCacheState.rankingDay === rankingDay
        ) {
          if (currentCacheState.prefetchStatus === "running") {
            currentCacheState.prefetchStatus = "canceled";
            currentCacheState.controller.abort();
          }
        }
        const nextRankingDay = currentRankingDay();
        if (nextRankingDay !== rankingDay) {
          setRankingDay(nextRankingDay);
          return;
        }
        if (profileKey === null) return;
        const queryKey = doomerboardRankingDayKey(profileKey, rankingDay);
        void (async () => {
          await cancelDoomerboardRankingDay(client, native, profileKey, rankingDay);
          await client.invalidateQueries({ queryKey, refetchType: "active" });
        })();
      }),
    );
  }, [client, hasNativeRuntime, native, profileKey, rankingDay, setRankingDay]);

  useEffect(() => {
    const previousCacheState = cacheState.current;
    const cacheChanged =
      previousCacheState !== null &&
      (!hasNativeRuntime ||
        profileKey === null ||
        previousCacheState.profileKey !== profileKey ||
        previousCacheState.rankingDay !== rankingDay);
    if (cacheChanged) {
      previousCacheState.controller.abort();
      void cancelDoomerboardRankingDay(
        client,
        native,
        previousCacheState.profileKey,
        previousCacheState.rankingDay,
      );
      client.removeQueries({
        queryKey: doomerboardRankingDayKey(
          previousCacheState.profileKey,
          previousCacheState.rankingDay,
        ),
      });
      cacheState.current = null;
    }
    if (!hasNativeRuntime || profileKey === null) return;
    if (cacheState.current === null) {
      cacheState.current = {
        controller: new AbortController(),
        prefetchStatus: "waiting",
        profileKey,
        rankingDay,
      };
    }
    if (!dataReady) return;
    const currentCacheState = cacheState.current;
    if (currentCacheState.prefetchStatus !== "waiting") return;
    currentCacheState.prefetchStatus = "running";
    void prefetchDoomerboardSelections({
      activeSelection,
      client,
      native,
      profileKey,
      rankingDay,
      signal: currentCacheState.controller.signal,
    }).then(() => {
      if (cacheState.current !== currentCacheState) return;
      currentCacheState.prefetchStatus = currentCacheState.controller.signal.aborted
        ? "canceled"
        : "complete";
    });
  }, [activeSelection, client, dataReady, hasNativeRuntime, native, profileKey, rankingDay]);

  useEffect(
    () => () => {
      cacheState.current?.controller.abort();
      cacheState.current = null;
    },
    [],
  );
}

function PanelScreen({
  doomerboardPort,
  hasNativeRuntime,
  presentation = {},
  stateDelivery,
}: PanelScreenProps) {
  const queryClient = useQueryClient();
  const [addTokenmaxxerFailure, setAddTokenmaxxerFailure] = useState<AddTokenmaxxerFailure | null>(
    null,
  );
  const [addTokenmaxxerOpen, setAddTokenmaxxerOpen] = useState(false);
  const [addTokenmaxxerInFlight, setAddTokenmaxxerInFlight] = useState(false);
  const [addTokenmaxxerRequests] = useState(createAddTokenmaxxerRequestGuard);
  const [doomerboardSelection, setDoomerboardSelection] = useState(defaultDoomerboardQuery);
  const [doomerboard] = useState(() => doomerboardPort ?? createTauriDoomerboardAdapter());
  const [rankingDay, setRankingDay] = useState(currentRankingDay);
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
  const profileKey =
    deliveryView.snapshot?.profile?.status === "ready"
      ? deliveryView.snapshot.profile.touchGrassId
      : null;
  useEffect(() => {
    let subscribedProfileKey = deliveredProfileKey(stateDelivery);
    return stateDelivery.subscribe(() => {
      const nextProfileKey = deliveredProfileKey(stateDelivery);
      if (nextProfileKey === subscribedProfileKey) return;
      subscribedProfileKey = nextProfileKey;
      addTokenmaxxerRequests.invalidate();
      setAddTokenmaxxerFailure(null);
      setAddTokenmaxxerOpen(false);
    });
  }, [addTokenmaxxerRequests, stateDelivery]);
  const doomerboardView = useQuery({
    ...createDoomerboardQueryOptions({
      native: doomerboard,
      profileKey: profileKey ?? "profile-unavailable",
      rankingDay,
      selection: doomerboardSelection,
    }),
    enabled: hasNativeRuntime && profileKey !== null,
  });
  useDoomerboardCache({
    activeSelection: doomerboardSelection,
    client: queryClient,
    dataReady: doomerboardView.data !== undefined,
    hasNativeRuntime,
    native: doomerboard,
    profileKey,
    rankingDay,
    setRankingDay,
  });

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
    const stop = retainAsyncSubscription(() =>
      doomerboard.subscribeFocus((focused) => focusManager.setFocused(focused)),
    );
    return () => {
      stop();
      focusManager.setFocused(undefined);
    };
  }, [doomerboard, hasNativeRuntime]);

  useEffect(() => {
    if (!hasNativeRuntime) return undefined;

    let active = true;
    let stop: (() => void) | undefined;
    void subscribeToPanelAddTokenmaxxer(() => {
      if (active) {
        addTokenmaxxerRequests.invalidate();
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
    doomerboardView.data?.status === "ready"
      ? doomerboardView.data.rows.map((row) => {
          const presentedRow: NonNullable<PanelViewProps["doomerboardRows"]>[number] = {
            displayName: row.displayName,
            rank: row.rank,
            tokenScore: compactTokenScore.format(row.tokenScore),
            touchGrassId: `#${row.touchGrassId}`,
          };
          if (row.apiEquivalentCostUsd !== null && row.apiEquivalentCostUsd !== undefined) {
            presentedRow.apiEquivalentCost = `≈ ${apiEquivalentCost.format(row.apiEquivalentCostUsd)}`;
          }
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
      addTokenmaxxerFailure={addTokenmaxxerFailure}
      addTokenmaxxerOpen={addTokenmaxxerOpen}
      addTokenmaxxerSubmitting={addTokenmaxxerInFlight}
      currentProfile={currentProfile}
      doomerboardLoading={presentation.doomerboardLoading ?? doomerboardView.isLoading}
      doomerboardRows={
        presentation.doomerboardRows ??
        (doomerboardSelection.audience === "global" ? nativeDoomerboardRows : undefined)
      }
      doomerboardSelection={doomerboardSelection}
      error={deliveryView.phase === "degraded"}
      nativeGlass
      onAddTokenmaxxer={(touchGrassId) => {
        const submissionProfileKey = profileKey;
        const request = addTokenmaxxerRequests.begin();
        if (request === null) return;
        setAddTokenmaxxerFailure(null);
        setAddTokenmaxxerInFlight(true);
        void (async () => {
          const outcome =
            hasNativeRuntime && submissionProfileKey !== null
              ? await addTokenmaxxer(doomerboard, submissionProfileKey, touchGrassId)
            : ({ status: "unavailable" } as const);
          const finishRequest = () => {
            const current = addTokenmaxxerRequests.finish(request);
            setAddTokenmaxxerInFlight(addTokenmaxxerRequests.inFlight());
            return current && deliveredProfileKey(stateDelivery) === submissionProfileKey;
          };
          if (outcome.status === "added" || outcome.status === "already-added") {
            const nextRankingDay = currentRankingDay();
            const nextSelection = { ...doomerboardSelection, audience: "mine" as const };
            if (
              submissionProfileKey !== null &&
              deliveredProfileKey(stateDelivery) === submissionProfileKey
            ) {
              await cancelDoomerboardAudience(
                queryClient,
                doomerboard,
                submissionProfileKey,
                "mine",
              );
              await queryClient.invalidateQueries({
                ...doomerboardProfileAudienceFilter(submissionProfileKey, "mine"),
                refetchType: "none",
              });
            }
            if (!finishRequest()) return;
            if (submissionProfileKey !== null) {
              void queryClient.prefetchQuery(
                createDoomerboardQueryOptions({
                  native: doomerboard,
                  profileKey: submissionProfileKey,
                  rankingDay: nextRankingDay,
                  selection: nextSelection,
                }),
              );
            }
            setRankingDay(nextRankingDay);
            setDoomerboardSelection(nextSelection);
            setAddTokenmaxxerOpen(false);
            return;
          }
          if (!finishRequest()) return;
          setAddTokenmaxxerFailure(outcome.status);
        })();
      }}
      onAddTokenmaxxerInputChange={() => {
        addTokenmaxxerRequests.invalidate();
        setAddTokenmaxxerFailure(null);
      }}
      onAddTokenmaxxerOpenChange={(open) => {
        addTokenmaxxerRequests.invalidate();
        setAddTokenmaxxerFailure(null);
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
              if (!hasNativeRuntime) {
                presentation.onUpdate?.();
                return;
              }
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
      updateActionPending={updateView.pendingAction !== null}
      updateState={updateState}
      usagePresentation={presentation.usagePresentation}
    />
  );
}

export { PanelScreen };
export type { PanelPresentation, PanelScreenProps };
