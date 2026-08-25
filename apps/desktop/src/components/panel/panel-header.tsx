import type { SanitizedDesktopState } from "@touchgrass/contracts";
import {
  Brand,
  Button,
  CircularProgressIcon,
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
  updateAction: PanelUpdateAction | null;
};

type PanelUpdateAction = {
  busy: boolean;
  indicator: "download" | "finalizing" | "progress" | "spinner";
  label: string;
  progressPercent?: number | null | undefined;
};

type PanelSyncStatus = SanitizedDesktopState["sync"]["status"];

function syncPresentation(
  error: boolean,
  refreshing: boolean,
  state: SanitizedDesktopState | null,
): {
  detailLabel: string | undefined;
  label: string;
  status: PanelSyncStatus | undefined;
} {
  if (refreshing) {
    return {
      detailLabel: undefined,
      label: "Syncing…",
      status: state?.sync.status,
    };
  }
  if (!state) {
    return {
      detailLabel: undefined,
      label: error ? "Sync unavailable" : "Connecting…",
      status: undefined,
    };
  }

  const status = state.sync.status;
  if (status === "pending") {
    return { detailLabel: undefined, label: "Syncing…", status };
  }

  const detailLabel = {
    "authority-rejected": "Active Mac transferred",
    offline: "Synchronization is offline",
    stale: "Synchronization is delayed",
    synced: undefined,
    unavailable: undefined,
  } satisfies Record<Exclude<PanelSyncStatus, "pending">, string | undefined>;

  return { detailLabel: detailLabel[status], label: "Live", status };
}

function UpdateActionIcon({ action }: { action: PanelUpdateAction }) {
  switch (action.indicator) {
    case "download":
      return <DownloadIcon aria-hidden="true" data-icon-source="Download04Icon" />;
    case "finalizing":
      return (
        <CircularProgressIcon
          aria-hidden="true"
          data-icon-source="CircularProgressIcon"
          progress={null}
          showCheck
        />
      );
    case "progress":
      return (
        <CircularProgressIcon
          aria-hidden="true"
          data-icon-source="CircularProgressIcon"
          progress={action.progressPercent}
        />
      );
    case "spinner":
      return (
        <CircularProgressIcon
          aria-hidden="true"
          data-icon-source="CircularProgressIcon"
          progress={null}
        />
      );
  }
}

function PanelHeader({
  error,
  onAddTokenmaxxer,
  onRefresh,
  onSettings,
  onUpdate,
  refreshing,
  state,
  updateAction,
}: PanelHeaderProps) {
  const sync = syncPresentation(error, refreshing, state);
  const updateStatusAnnouncement =
    updateAction?.indicator === "progress"
      ? "Downloading signed update"
      : (updateAction?.label ?? "");

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
          {sync.detailLabel ? <span className="sr-only">. {sync.detailLabel}</span> : null}
        </small>
      </div>

      <div className="ml-2 flex items-center gap-1">
        <span aria-atomic="true" aria-live="polite" className="sr-only" data-slot="update-status">
          {updateStatusAnnouncement}
        </span>
        {updateAction ? (
          <Button
            aria-busy={updateAction.busy || undefined}
            aria-label={updateAction.label}
            className={updateAction.busy ? "disabled:opacity-100" : undefined}
            data-slot="update-action"
            disabled={updateAction.busy}
            onClick={onUpdate}
            size="icon"
            title={updateAction.label}
            type="button"
          >
            <UpdateActionIcon action={updateAction} />
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
            {sync.detailLabel ? (
              <PanelMenuItem
                aria-label={sync.detailLabel}
                className="text-pearl-muted"
                data-slot="sync-status"
                disabled
              >
                {sync.detailLabel}
              </PanelMenuItem>
            ) : null}
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
export type { PanelUpdateAction };
