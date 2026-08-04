import type { SanitizedDesktopState } from "@touchgrass/contracts";
import { AddFriendDialog, PanelShell } from "@touchgrass/ui";
import { useRef, useState } from "react";

import type {
  DoomerboardPreviewRow,
  UsagePreview,
} from "../../previewFixtures";
import { LoadingPanel } from "./loading-panel";
import { DoomerboardPreview } from "./doomerboard-preview";
import { PanelHeader } from "./panel-header";
import { ProviderCard } from "./provider-card";
import { UsageOverview } from "./usage-overview";

type PanelViewProps = {
  doomerboardPreviewRows?: readonly DoomerboardPreviewRow[] | undefined;
  error: boolean;
  nativeGlass?: boolean;
  onRefresh: () => void;
  onSettings: () => void;
  onUpdate?: (() => void) | undefined;
  refreshing: boolean;
  state: SanitizedDesktopState | null;
  tokenmaxxerPreviewRows?: readonly DoomerboardPreviewRow[] | undefined;
  updateAvailable?: boolean | undefined;
  usagePreview?: UsagePreview | undefined;
};

function PanelView({
  doomerboardPreviewRows,
  error,
  nativeGlass = false,
  onRefresh,
  onSettings,
  onUpdate = () => undefined,
  refreshing,
  state,
  tokenmaxxerPreviewRows,
  updateAvailable = false,
  usagePreview,
}: PanelViewProps) {
  const [addFriendOpen, setAddFriendOpen] = useState(false);
  const panelContainerRef = useRef<HTMLElement>(null);

  return (
    <>
      <PanelShell glass={nativeGlass} ref={panelContainerRef}>
        <PanelHeader
          error={error}
          onAddFriend={() => setAddFriendOpen(true)}
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
            className="border-b border-cream-line bg-cream-surface-soft p-5 contrast-more:border-cream-ink contrast-more:bg-cream-highlight"
            role="alert"
          >
            <strong className="text-[14px]">Nothing invented.</strong>
            <p className="mt-1.5 mb-0 text-[11px] leading-5 text-cream-muted contrast-more:text-cream-ink">
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
          <UsageOverview preview={usagePreview} usage={state.usage.codex} />
          <DoomerboardPreview
            onAddFriend={() => setAddFriendOpen(true)}
            previewRows={doomerboardPreviewRows}
            tokenmaxxerPreviewRows={tokenmaxxerPreviewRows}
          />
        </>
      )}
      </PanelShell>
      <AddFriendDialog
        onOpenChange={setAddFriendOpen}
        open={addFriendOpen}
        portalContainer={panelContainerRef.current}
      />
    </>
  );
}

export { PanelView };
export type { PanelViewProps };
