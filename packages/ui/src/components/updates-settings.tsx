import type { UpdateState } from "@touchgrass/contracts";

import { Button } from "./button";
import { CircularProgressIcon } from "./circular-progress-icon";
import { SettingsToggleRow } from "./settings-toggle-row";

type UpdatesSettingsProps = {
  actionPending?: boolean | undefined;
  autoUpdates: boolean | null;
  onAutoUpdatesChange?: ((value: boolean) => void) | undefined;
  onCheckForUpdates?: (() => void) | undefined;
  onInstall?: (() => void) | undefined;
  onOpenLatestDmg?: (() => void) | undefined;
  onOpenSource?: (() => void) | undefined;
  onRetry?: (() => void) | undefined;
  state: UpdateState | null;
};

function updateSummary(state: UpdateState | null, actionPending = false) {
  if (state === null) return "Update state unavailable.";
  const update = state.update;
  if (actionPending) {
    if (update.status === "available") return "Starting signed update…";
    if (update.status === "failed") return "Retrying update…";
    if (update.status === "idle" || update.status === "upToDate") {
      return "Checking the stable channel…";
    }
  }
  switch (update.status) {
    case "available":
      return state.onlineFeaturesPaused
        ? `Version ${update.version} is required for online features.`
        : `Version ${update.version} is ready.`;
    case "checking":
      return "Checking the stable channel…";
    case "downloading":
      return update.progressPercent === null || update.progressPercent === undefined
        ? "Downloading signed update…"
        : `Downloading signed update · ${update.progressPercent}%`;
    case "failed":
      return state.onlineFeaturesPaused
        ? "Required update not installed."
        : "Update not installed.";
    case "installing":
      return "Installing and relaunching…";
    case "unavailable":
      return "Updater unavailable in this build.";
    case "upToDate":
      return "TouchGrassBar is up to date.";
    case "idle":
      return state.onlineFeaturesPaused ? "Update required for online features." : "Stable channel";
  }
}

function updateStatusAnnouncement(state: UpdateState | null, actionPending: boolean) {
  return state?.update.status === "downloading"
    ? "Downloading signed update."
    : updateSummary(state, actionPending);
}

function UpdatesSettings({
  actionPending = false,
  autoUpdates,
  onAutoUpdatesChange,
  onCheckForUpdates,
  onInstall,
  onOpenLatestDmg,
  onOpenSource,
  onRetry,
  state,
}: UpdatesSettingsProps) {
  const status = state?.update.status;
  const downloadProgress =
    state?.update.status === "downloading" ? state.update.progressPercent : null;
  const primaryAction =
    status === "available" ? onInstall : status === "failed" ? onRetry : onCheckForUpdates;
  const nativeBusy = status === "checking" || status === "downloading" || status === "installing";
  const checkAction =
    status !== "available" &&
    status !== "failed" &&
    status !== "downloading" &&
    status !== "installing";
  const primaryLabel =
    actionPending && !nativeBusy
      ? status === "available"
        ? "Downloading…"
        : status === "failed"
          ? "Retrying…"
          : "Checking…"
      : status === "available"
        ? "Install & Relaunch"
        : status === "failed"
          ? "Retry"
          : status === "checking"
            ? "Checking…"
            : status === "downloading"
              ? "Downloading…"
              : status === "installing"
                ? "Relaunching…"
                : "Check now";
  const primaryBusy = actionPending || nativeBusy;
  const primaryIndicator =
    status === "downloading" ? (
      <CircularProgressIcon
        aria-hidden="true"
        data-icon-source="CircularProgressIcon"
        progress={downloadProgress}
      />
    ) : status === "installing" ? (
      <CircularProgressIcon
        aria-hidden="true"
        data-icon-source="CircularProgressIcon"
        progress={null}
        showCheck
      />
    ) : actionPending && !checkAction ? (
      <CircularProgressIcon
        aria-hidden="true"
        data-icon-source="CircularProgressIcon"
        progress={null}
      />
    ) : null;
  const recovery = status === "failed";

  return (
    <div className="grid gap-3" data-slot="updates-settings">
      <div
        className="overflow-hidden rounded-[12px] border border-sheet-row-border bg-sheet-row"
        data-slot="update-settings-group"
      >
        <div className="flex items-center justify-between gap-6 px-4 py-3.5">
          <span>
            <strong className="block text-[12px]">
              Version {state?.currentVersion ?? "unavailable"}
            </strong>
            <small className="mt-0.5 block text-[9px] text-sheet-muted">
              {updateSummary(state, actionPending)}
            </small>
            <span
              aria-atomic="true"
              aria-live="polite"
              className="sr-only"
              data-slot="update-status"
            >
              {updateStatusAnnouncement(state, actionPending)}
            </span>
          </span>
          <Button
            aria-busy={primaryBusy || undefined}
            className={primaryBusy ? "h-7 py-0 disabled:opacity-100" : "h-7 py-0"}
            disabled={primaryAction === undefined || primaryBusy || status === "unavailable"}
            onClick={primaryAction}
            type="button"
            variant="ghost"
          >
            {primaryIndicator}
            {checkAction ? (
              <span className="inline-grid" data-slot="update-check-labels">
                <span
                  aria-hidden={primaryBusy ? true : undefined}
                  className={
                    primaryBusy ? "invisible col-start-1 row-start-1" : "col-start-1 row-start-1"
                  }
                >
                  Check now
                </span>
                <span
                  aria-hidden={primaryBusy ? undefined : true}
                  className={
                    primaryBusy ? "col-start-1 row-start-1" : "invisible col-start-1 row-start-1"
                  }
                >
                  Checking…
                </span>
              </span>
            ) : (
              primaryLabel
            )}
          </Button>
        </div>
        <SettingsToggleRow
          checked={autoUpdates ?? false}
          disabled={autoUpdates === null}
          grouped
          label="Check automatically"
          onCheckedChange={onAutoUpdatesChange}
          {...(autoUpdates === null ? { description: "Not connected in this build." } : {})}
        />
      </div>
      <div className="flex items-center justify-between gap-6 border-t border-sheet-line px-4 pt-3">
        <span>
          <strong className="block text-[11px]">
            {recovery ? "Update recovery" : "Open source"}
          </strong>
          <small className="mt-0.5 block text-[9px] text-sheet-muted">
            {recovery
              ? "Use the latest DMG if Retry does not work."
              : "Source code, releases, and issues on GitHub."}
          </small>
        </span>
        <Button
          disabled={recovery ? onOpenLatestDmg === undefined : onOpenSource === undefined}
          onClick={recovery ? onOpenLatestDmg : onOpenSource}
          type="button"
          variant="ghost"
        >
          {recovery ? "Download latest DMG ↗" : "View on GitHub ↗"}
        </Button>
      </div>
    </div>
  );
}

export { UpdatesSettings };
export type { UpdatesSettingsProps };
