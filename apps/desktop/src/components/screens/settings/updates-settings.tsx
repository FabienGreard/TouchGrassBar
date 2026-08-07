import type { UpdateState } from "@touchgrass/contracts";
import { Button } from "@touchgrass/ui";

import { SettingsToggleRow } from "./settings-toggle-row";

type UpdatesSettingsProps = {
  autoUpdates: boolean | null;
  onAutoUpdatesChange?: ((value: boolean) => void) | undefined;
  onCheckForUpdates?: (() => void) | undefined;
  onInstall?: (() => void) | undefined;
  onOpenLatestDmg?: (() => void) | undefined;
  onOpenSource?: (() => void) | undefined;
  onRetry?: (() => void) | undefined;
  state: UpdateState | null;
};

function updateSummary(state: UpdateState | null) {
  if (state === null) return "Development build";
  const update = state.update;
  switch (update.status) {
    case "available":
      return `Version ${update.version} is ready.`;
    case "checking":
      return "Checking the stable channel…";
    case "downloading":
      return update.progressPercent === null
        ? "Downloading signed update…"
        : `Downloading signed update · ${update.progressPercent}%`;
    case "failed":
      return "Update not installed.";
    case "installing":
      return "Installing and relaunching…";
    case "unavailable":
      return "Updater unavailable in this build.";
    case "upToDate":
      return "TouchGrassBar is up to date.";
    case "idle":
      return state.onlineFeaturesPaused
        ? "Update required for online features."
        : "Stable channel";
  }
}

function UpdatesSettings({
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
  const primaryAction =
    status === "available"
      ? onInstall
      : status === "failed"
        ? onRetry
        : onCheckForUpdates;
  const primaryLabel =
    status === "available"
      ? "Install & Relaunch"
      : status === "failed"
        ? "Retry"
        : "Check now";
  const primaryBusy =
    status === "checking" ||
    status === "downloading" ||
    status === "installing";
  const recovery = status === "failed";

  return (
    <div className="grid gap-3" data-slot="updates-settings">
      <div className="flex items-center justify-between gap-6 rounded-[12px] bg-white/38 px-4 py-3.5">
        <span>
          <strong className="block text-[12px]">
            Version {state?.currentVersion ?? "0.0.0"}
          </strong>
          <small className="mt-0.5 block text-[9px] text-sheet-muted">
            {updateSummary(state)}
          </small>
        </span>
        <Button
          disabled={
            primaryAction === undefined ||
            primaryBusy ||
            status === "unavailable"
          }
          onClick={primaryAction}
          type="button"
          variant="ghost"
        >
          {primaryLabel}
        </Button>
      </div>
      <SettingsToggleRow
        checked={autoUpdates ?? false}
        disabled={autoUpdates === null}
        label="Check automatically"
        onCheckedChange={onAutoUpdatesChange}
        {...(autoUpdates === null
          ? { description: "Not connected in this build." }
          : {})}
      />
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
          disabled={
            recovery
              ? onOpenLatestDmg === undefined
              : onOpenSource === undefined
          }
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
