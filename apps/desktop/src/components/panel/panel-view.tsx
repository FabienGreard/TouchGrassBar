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
import {
  UsageOverview,
  type UsagePresentation,
} from "@/components/panel/usage-overview";

function updateActionLabel(updateState: UpdateState | null) {
  if (updateState?.update.status === "failed") {
    return updateState.onlineFeaturesPaused
      ? "Retry required update"
      : "Retry update";
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
  currentProfile?: CurrentProfile | null | undefined;
  doomerboardRows?: readonly DoomerboardRow[] | undefined;
  error: boolean;
  expanded?: boolean | undefined;
  nativeGlass?: boolean;
  onAddTokenmaxxerOpenChange?: ((open: boolean) => void) | undefined;
  onExpandedChange?: ((expanded: boolean) => void) | undefined;
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
  currentProfile,
  doomerboardRows,
  error,
  expanded = false,
  nativeGlass = false,
  onAddTokenmaxxerOpenChange = () => undefined,
  onExpandedChange = () => undefined,
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
        className={expanded ? "expanded-board-surface w-[620px]" : undefined}
        data-expanded={expanded}
        data-glass={nativeGlass && !expanded ? "true" : "false"}
        glass={nativeGlass && !expanded}
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
            {expanded ? null : (
              <>
                <div>
                  {visibleProviders.map((provider) => (
                    <ProviderCard
                      key={provider.provider}
                      presentation={provider}
                    />
                  ))}
                </div>
                <UsageOverview
                  presentation={usagePresentation}
                  topModelUsage={state.topModelUsage}
                  usage={state.combinedUsage}
                />
              </>
            )}
            <Doomerboard
              currentProfile={currentProfile}
              expanded={expanded}
              key="doomerboard"
              onAddTokenmaxxer={() => onAddTokenmaxxerOpenChange(true)}
              onExpandedChange={onExpandedChange}
              providers={visibleProviders}
              rows={doomerboardRows}
              tokenmaxxerRows={tokenmaxxerRows}
            />
          </>
        )}
      </PanelShell>
      <AddTokenmaxxerDialog
        onOpenChange={onAddTokenmaxxerOpenChange}
        open={addTokenmaxxerOpen}
        portalContainer={panelContainerRef.current}
      />
    </>
  );
}

export { PanelView };
export type { PanelViewProps };
