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

type PanelHeaderProps = {
  error: boolean;
  onAddTokenmaxxer: () => void;
  onRefresh: () => void;
  onSettings: () => void;
  onUpdate: () => void;
  refreshing: boolean;
  state: SanitizedDesktopState | null;
  updateAvailable: boolean;
};

function syncLabel(error: boolean, state: SanitizedDesktopState | null) {
  if (error) return "Local state unavailable";
  if (!state) return "Opening local cache";
  if (state.sync.status === "synced") return "Synced locally";
  if (state.sync.status === "pending") return "Sync pending";
  if (state.sync.status === "stale") return "Local snapshot is stale";
  return "Sync unavailable";
}

function PanelHeader({
  error,
  onAddTokenmaxxer,
  onRefresh,
  onSettings,
  onUpdate,
  refreshing,
  state,
  updateAvailable,
}: PanelHeaderProps) {
  return (
    <header className="flex items-center justify-between border-b border-pearl-line bg-panel-header px-4 pt-[15px] pb-3 contrast-more:border-pearl-ink">
      <div className="flex min-w-0 items-center gap-2.5">
        <Brand />
        <small className="truncate border-l border-pearl-line pl-2.5 text-[10px] text-pearl-muted contrast-more:border-pearl-ink contrast-more:text-pearl-ink">
          {syncLabel(error, state)}
        </small>
      </div>

      <div className="ml-2 flex items-center gap-1">
        {updateAvailable ? (
          <Button
            aria-label="Install update and relaunch"
            data-slot="update-action"
            onClick={onUpdate}
            size="icon"
            title="Install update and relaunch"
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
              {refreshing ? "Forcing sync…" : "Force sync"}
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
