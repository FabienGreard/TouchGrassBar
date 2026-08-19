import type {
  ProviderPresentation,
  SanitizedDesktopState,
  UpdateState,
} from "@touchgrass/contracts";
import { PanelShell } from "@touchgrass/ui";
import { useRef } from "react";

import { AddTokenmaxxerDialog } from "@/components/dialogs/add-tokenmaxxer-dialog";
import {
  Doomerboard,
  type CurrentProfile,
  type DoomerboardRow,
} from "@/components/panel/doomerboard";
import { LoadingPanel } from "@/components/panel/loading-panel";
import { PanelHeader } from "@/components/panel/panel-header";
import { ProviderCard } from "@/components/panel/provider-card";
import { UsageOverview, type UsagePresentation } from "@/components/panel/usage-overview";
import {
  defaultDoomerboardQuery,
  type DoomerboardQuery,
} from "@/native-state/doomerboard-delivery";

function updateActionLabel(updateState: UpdateState | null) {
  if (updateState?.update.status === "failed") {
    return updateState.onlineFeaturesPaused ? "Retry required update" : "Retry update";
  }
  if (updateState?.update.status === "available") {
    return updateState.onlineFeaturesPaused
      ? "Install required update and relaunch"
      : "Install update and relaunch";
  }
  return null;
}

type PanelViewProps = {
  addTokenmaxxerOpen?: boolean | undefined;
  addTokenmaxxerSubmitting?: boolean | undefined;
  currentProfile?: CurrentProfile | null | undefined;
  doomerboardRows?: readonly DoomerboardRow[] | undefined;
  doomerboardSelection?: DoomerboardQuery | undefined;
  error: boolean;
  nativeGlass?: boolean;
  onAddTokenmaxxer?: ((touchGrassId: string) => void) | undefined;
  onAddTokenmaxxerInputChange?: (() => void) | undefined;
  onAddTokenmaxxerOpenChange?: ((open: boolean) => void) | undefined;
  onDoomerboardSelectionChange?: ((selection: DoomerboardQuery) => void) | undefined;
  onRefresh: () => void;
  onSettings: () => void;
  onUpdate?: (() => void) | undefined;
  refreshing: boolean;
  state: SanitizedDesktopState | null;
  tokenmaxxerRows?: readonly DoomerboardRow[] | undefined;
  updateState?: UpdateState | null | undefined;
  usagePresentation?: UsagePresentation | undefined;
};

function PanelView({
  addTokenmaxxerOpen = false,
  addTokenmaxxerSubmitting = false,
  currentProfile,
  doomerboardRows,
  doomerboardSelection = defaultDoomerboardQuery,
  error,
  nativeGlass = false,
  onAddTokenmaxxer = () => undefined,
  onAddTokenmaxxerInputChange = () => undefined,
  onAddTokenmaxxerOpenChange = () => undefined,
  onDoomerboardSelectionChange = () => undefined,
  onRefresh,
  onSettings,
  onUpdate = () => undefined,
  refreshing,
  state,
  tokenmaxxerRows,
  updateState = null,
  usagePresentation,
}: PanelViewProps) {
  const panelContainerRef = useRef<HTMLElement>(null);
  const visibleProviders: ProviderPresentation[] = state?.providers ?? [];

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
          updateActionLabel={updateActionLabel(updateState)}
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
              onAddTokenmaxxer={() => onAddTokenmaxxerOpenChange(true)}
              onSelectionChange={onDoomerboardSelectionChange}
              providers={visibleProviders}
              rows={doomerboardRows}
              selection={doomerboardSelection}
              tokenmaxxerRows={tokenmaxxerRows}
            />
          </>
        )}
      </PanelShell>
      <AddTokenmaxxerDialog
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
