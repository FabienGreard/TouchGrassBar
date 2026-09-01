import type {
  ProviderPresentation,
  SanitizedDesktopState,
  UpdateState,
} from "@touchgrass/contracts";
import { PanelShell } from "@touchgrass/ui";
import { useRef } from "react";

import { AddTokenmaxxerDialog } from "@/components/dialogs/add-tokenmaxxer-dialog";
import type { AddTokenmaxxerFailure } from "@/components/dialogs/add-tokenmaxxer";
import {
  Doomerboard,
  type CurrentProfile,
  type DoomerboardRow,
} from "@/components/panel/doomerboard";
import { LoadingPanel } from "@/components/panel/loading-panel";
import { PanelHeader, type PanelUpdateAction } from "@/components/panel/panel-header";
import { ProviderCard } from "@/components/panel/provider-card";
import { UsageOverview, type UsagePresentation } from "@/components/panel/usage-overview";
import { defaultDoomerboardQuery, type DoomerboardQuery } from "@/native-state/doomerboard-query";

function updateActionPresentation(
  updateState: UpdateState | null,
  updateActionPending: boolean,
): PanelUpdateAction | null {
  const update = updateState?.update;
  if (!update) return null;

  if (updateActionPending) {
    if (update.status === "available") {
      return { busy: true, indicator: "spinner", label: "Starting update download" };
    }
    if (update.status === "failed") {
      return { busy: true, indicator: "spinner", label: "Retrying update" };
    }
  }

  switch (update.status) {
    case "available":
      return {
        busy: false,
        indicator: "download",
        label: updateState.onlineFeaturesPaused
          ? "Install required update and relaunch"
          : "Install update and relaunch",
      };
    case "checking":
      return { busy: true, indicator: "spinner", label: "Checking for updates" };
    case "downloading":
      return {
        busy: true,
        indicator: "progress",
        label:
          update.progressPercent === null || update.progressPercent === undefined
            ? "Downloading signed update"
            : `Downloading signed update, ${update.progressPercent} percent`,
        progressPercent: update.progressPercent,
      };
    case "failed":
      return {
        busy: false,
        indicator: "download",
        label: updateState.onlineFeaturesPaused ? "Retry required update" : "Retry update",
      };
    case "installing":
      return {
        busy: true,
        indicator: "finalizing",
        label: "Installing and relaunching",
      };
    case "idle":
    case "unavailable":
    case "upToDate":
      return null;
  }
}

type PanelViewProps = {
  addTokenmaxxerFailure?: AddTokenmaxxerFailure | null | undefined;
  addTokenmaxxerOpen?: boolean | undefined;
  addTokenmaxxerSubmitting?: boolean | undefined;
  currentProfile?: CurrentProfile | null | undefined;
  doomerboardLoading?: boolean | undefined;
  doomerboardRows?: readonly DoomerboardRow[] | undefined;
  doomerboardSelection?: DoomerboardQuery | undefined;
  error: boolean;
  nativeGlass?: boolean;
  onAddTokenmaxxer?: ((touchGrassId: string) => void) | undefined;
  onAddTokenmaxxerInputChange?: (() => void) | undefined;
  onAddTokenmaxxerOpenChange?: ((open: boolean) => void) | undefined;
  onDoomerboardSelectionChange?: ((selection: DoomerboardQuery) => void) | undefined;
  onDoomerboardSelectionIntent?: ((selection: DoomerboardQuery) => void) | undefined;
  onRefresh: () => void;
  onSettings: () => void;
  onUpdate?: (() => void) | undefined;
  refreshing: boolean;
  state: SanitizedDesktopState | null;
  tokenmaxxerRows?: readonly DoomerboardRow[] | undefined;
  updateActionPending?: boolean | undefined;
  updateState?: UpdateState | null | undefined;
  usagePresentation?: UsagePresentation | undefined;
};

function PanelView({
  addTokenmaxxerFailure = null,
  addTokenmaxxerOpen = false,
  addTokenmaxxerSubmitting = false,
  currentProfile,
  doomerboardLoading = false,
  doomerboardRows,
  doomerboardSelection = defaultDoomerboardQuery,
  error,
  nativeGlass = false,
  onAddTokenmaxxer = () => undefined,
  onAddTokenmaxxerInputChange = () => undefined,
  onAddTokenmaxxerOpenChange = () => undefined,
  onDoomerboardSelectionChange = () => undefined,
  onDoomerboardSelectionIntent = () => undefined,
  onRefresh,
  onSettings,
  onUpdate = () => undefined,
  refreshing,
  state,
  tokenmaxxerRows,
  updateActionPending = false,
  updateState = null,
  usagePresentation,
}: PanelViewProps) {
  const panelContainerRef = useRef<HTMLElement>(null);
  const visibleProviders: ProviderPresentation[] = state?.providers ?? [];
  const updateAction = updateActionPresentation(updateState, updateActionPending);

  return (
    <>
      <PanelShell
        data-glass={nativeGlass ? "true" : "false"}
        glass={nativeGlass}
        ref={panelContainerRef}
      >
        <PanelHeader
          error={error}
          onAddTokenmaxxer={() => onAddTokenmaxxerOpenChange(true)}
          onRefresh={onRefresh}
          onSettings={onSettings}
          onUpdate={onUpdate}
          refreshing={refreshing}
          state={state}
          updateAction={updateAction}
        />

        {!state ? (
          <LoadingPanel loading={!error} />
        ) : (
          <>
            <div>
              {visibleProviders.map((provider) => (
                <ProviderCard key={provider.provider} presentation={provider} />
              ))}
            </div>
            <UsageOverview
              presentation={usagePresentation}
              topModelUsage={state.topModelUsage}
              usage={state.combinedUsage}
            />
            <Doomerboard
              currentProfile={currentProfile}
              key="doomerboard"
              loading={doomerboardLoading}
              onAddTokenmaxxer={() => onAddTokenmaxxerOpenChange(true)}
              onSelectionChange={onDoomerboardSelectionChange}
              onSelectionIntent={onDoomerboardSelectionIntent}
              providers={visibleProviders}
              rows={doomerboardRows}
              selection={doomerboardSelection}
              tokenmaxxerRows={tokenmaxxerRows}
            />
          </>
        )}
      </PanelShell>
      <AddTokenmaxxerDialog
        failure={addTokenmaxxerFailure}
        key={addTokenmaxxerOpen ? "open" : "closed"}
        onAddTokenmaxxer={onAddTokenmaxxer}
        onInputChange={onAddTokenmaxxerInputChange}
        onOpenChange={onAddTokenmaxxerOpenChange}
        open={addTokenmaxxerOpen}
        portalContainer={panelContainerRef.current}
        submitting={addTokenmaxxerSubmitting}
      />
    </>
  );
}

export { PanelView };
export type { PanelViewProps };
