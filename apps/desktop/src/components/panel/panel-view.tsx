import type { SanitizedDesktopState } from "@touchgrass/contracts";
import { PanelShell } from "@touchgrass/ui";
import { useRef, useState } from "react";

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

type PanelViewProps = {
  currentProfile?: CurrentProfile | null | undefined;
  doomerboardRows?: readonly DoomerboardRow[] | undefined;
  error: boolean;
  nativeGlass?: boolean;
  onRefresh: () => void;
  onSettings: () => void;
  onUpdate?: (() => void) | undefined;
  refreshing: boolean;
  state: SanitizedDesktopState | null;
  tokenmaxxerRows?: readonly DoomerboardRow[] | undefined;
  updateAvailable?: boolean | undefined;
  usagePresentation?: UsagePresentation | undefined;
};

function PanelView({
  currentProfile,
  doomerboardRows,
  error,
  nativeGlass = false,
  onRefresh,
  onSettings,
  onUpdate = () => undefined,
  refreshing,
  state,
  tokenmaxxerRows,
  updateAvailable = false,
  usagePresentation,
}: PanelViewProps) {
  const [addTokenmaxxerOpen, setAddTokenmaxxerOpen] = useState(false);
  const panelContainerRef = useRef<HTMLElement>(null);

  return (
    <>
      <PanelShell glass={nativeGlass} ref={panelContainerRef}>
        <PanelHeader
          error={error}
          onAddTokenmaxxer={() => setAddTokenmaxxerOpen(true)}
          onRefresh={onRefresh}
          onSettings={onSettings}
          onUpdate={onUpdate}
          refreshing={refreshing}
          state={state}
          updateAvailable={updateAvailable}
        />

        {!state ? (
          error ? (
            <section
              className="border-b border-pearl-line bg-pearl-surface-soft p-5 contrast-more:border-pearl-ink contrast-more:bg-pearl-highlight"
              role="alert"
            >
              <strong className="text-[14px]">Nothing invented.</strong>
              <p className="mt-1.5 mb-0 text-[11px] leading-5 text-pearl-muted contrast-more:text-pearl-ink">
                The native snapshot is unavailable. No missing value has been
                counted as zero.
              </p>
            </section>
          ) : (
            <LoadingPanel />
          )
        ) : (
          <>
            <div>
              {state.providers.map((provider) => (
                <ProviderCard key={provider.provider} provider={provider} />
              ))}
            </div>
            <UsageOverview
              presentation={usagePresentation}
              usage={state.usage.codex}
            />
            <Doomerboard
              currentProfile={currentProfile}
              onAddTokenmaxxer={() => setAddTokenmaxxerOpen(true)}
              rows={doomerboardRows}
              tokenmaxxerRows={tokenmaxxerRows}
            />
          </>
        )}
      </PanelShell>
      <AddTokenmaxxerDialog
        onOpenChange={setAddTokenmaxxerOpen}
        open={addTokenmaxxerOpen}
        portalContainer={panelContainerRef.current}
      />
    </>
  );
}

export { PanelView };
export type { PanelViewProps };
