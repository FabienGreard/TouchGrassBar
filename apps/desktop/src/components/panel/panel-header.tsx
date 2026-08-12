import type { SanitizedDesktopState } from "@touchgrass/contracts";
import {
  Brand,
  Button,
  DownloadIcon,
  EllipsisIcon,
  InviteIcon,
  PanelMenu,
  PanelMenuContent,
  PanelMenuItem,
  PanelMenuTrigger,
  RefreshIcon,
  SettingsIcon,
} from "@touchgrass/ui";

import { refreshActionLabel } from "@/components/panel/panel-header-copy";

type PanelHeaderProps = {
  error: boolean;
  onAddTokenmaxxer: () => void;
  onRefresh: () => void;
  onSettings: () => void;
  onUpdate: () => void;
  refreshing: boolean;
  state: SanitizedDesktopState | null;
  updateActionLabel: string | null;
};

type PanelSyncStatus = SanitizedDesktopState["sync"]["status"];

function syncPresentation(
  error: boolean,
  refreshing: boolean,
  state: SanitizedDesktopState | null,
): { label: string; status: PanelSyncStatus | undefined } {
  if (refreshing) {
    return { label: "Syncing…", status: state?.sync.status };
  }
  if (!state) {
    return {
      label: error ? "Sync unavailable" : "Connecting…",
      status: undefined,
    };
  }

  const status = state.sync.status;
  return { label: "Live", status };
}

function PanelHeader({
  error,
  onAddTokenmaxxer,
  onRefresh,
  onSettings,
  onUpdate,
  refreshing,
  state,
  updateActionLabel,
}: PanelHeaderProps) {
  const sync = syncPresentation(error, refreshing, state);

  return (
    <header className="flex items-center justify-between border-b border-pearl-line bg-panel-header px-4 pt-[15px] pb-3 contrast-more:border-pearl-ink">
      <div className="flex min-w-0 items-center gap-2.5">
        <Brand />
        <small
          aria-live="polite"
          className="truncate border-l border-pearl-line pl-2.5 text-[10px] text-pearl-muted contrast-more:border-pearl-ink contrast-more:text-pearl-ink"
          data-sync-status={sync.status}
        >
          {sync.label}
        </small>
      </div>

      <div className="ml-2 flex items-center gap-1">
        {updateActionLabel ? (
          <Button
            aria-label={updateActionLabel}
            data-slot="update-action"
            onClick={onUpdate}
            size="icon"
            title={updateActionLabel}
            type="button"
          >
            <DownloadIcon
              aria-hidden="true"
              data-icon-source="Download04Icon"
            />
          </Button>
        ) : null}
        <PanelMenu>
          <PanelMenuTrigger asChild>
            <Button
              aria-label="Open panel menu"
              size="icon"
              title="Open panel menu"
              type="button"
              variant="ghost"
            >
              <EllipsisIcon aria-hidden="true" size={19} />
            </Button>
          </PanelMenuTrigger>
          <PanelMenuContent align="end" sideOffset={7}>
            <PanelMenuItem disabled={refreshing} onSelect={onRefresh}>
              <RefreshIcon aria-hidden="true" spin={refreshing} />
              {refreshActionLabel(refreshing)}
            </PanelMenuItem>
            <PanelMenuItem onSelect={onAddTokenmaxxer}>
              <InviteIcon aria-hidden="true" />
              Add a Tokenmaxxer…
            </PanelMenuItem>
            <PanelMenuItem onSelect={onSettings}>
              <SettingsIcon aria-hidden="true" />
              Settings…
            </PanelMenuItem>
          </PanelMenuContent>
        </PanelMenu>
      </div>
    </header>
  );
}

export { PanelHeader };
