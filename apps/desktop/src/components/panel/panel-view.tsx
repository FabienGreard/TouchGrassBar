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
import { UpdateExperience, type UpdateActions } from "@/update/update-experience";

type PanelViewProps = UpdateActions & {
  addTokenmaxxerOpen?: boolean | undefined;
  currentProfile?: CurrentProfile | null | undefined;
  doomerboardRows?: readonly DoomerboardRow[] | undefined;
  error: boolean;
  nativeGlass?: boolean;
  onAddTokenmaxxerOpenChange?: ((open: boolean) => void) | undefined;
  onRefresh: () => void;
  onSettings: () => void;
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
  nativeGlass = false,
  onAddTokenmaxxerOpenChange = () => undefined,
  onRefresh,
  onSettings,
  onCheck,
  onDefer,
  onInstall,
  onOpenLatestDmg,
  onRetry,
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
      <PanelShell glass={nativeGlass} ref={panelContainerRef}>
        <PanelHeader
          error={error}
          onAddTokenmaxxer={() => onAddTokenmaxxerOpenChange(true)}
          onRefresh={onRefresh}
          onSettings={onSettings}
          refreshing={refreshing}
          state={state}
        />
        <UpdateExperience
          onCheck={onCheck}
          onDefer={onDefer}
          onInstall={onInstall}
          onOpenLatestDmg={onOpenLatestDmg}
          onRetry={onRetry}
          state={updateState}
          surface="panel"
        />

        {!state ? (
          <LoadingPanel loading={!error} />
        ) : (
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
              usage={state.combinedUsage}
            />
            <Doomerboard
              currentProfile={currentProfile}
              onAddTokenmaxxer={() => onAddTokenmaxxerOpenChange(true)}
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
