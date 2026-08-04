import { Button } from "@touchgrass/ui";

import { SettingsToggleRow } from "./settings-toggle-row";

function UpdatesSettings({
  autoUpdates,
  onAutoUpdatesChange,
  onCheckForUpdates,
  onOpenSource,
}: {
  autoUpdates: boolean | null;
  onAutoUpdatesChange?: ((value: boolean) => void) | undefined;
  onCheckForUpdates?: (() => void) | undefined;
  onOpenSource?: (() => void) | undefined;
}) {
  return (
    <div className="grid gap-3" data-slot="updates-settings">
      <div className="flex items-center justify-between gap-6 rounded-[12px] bg-white/38 px-4 py-3.5">
        <span>
          <strong className="block text-[12px]">Version 0.0.0</strong>
          <small className="mt-0.5 block text-[9px] text-sheet-muted">
            Development build
          </small>
        </span>
        <Button
          disabled={onCheckForUpdates === undefined}
          onClick={onCheckForUpdates}
          type="button"
          variant="ghost"
        >
          Check now
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
          <strong className="block text-[11px]">Open source</strong>
          <small className="mt-0.5 block text-[9px] text-sheet-muted">
            Source code, releases, and issues on GitHub.
          </small>
        </span>
        <Button
          disabled={onOpenSource === undefined}
          onClick={onOpenSource}
          type="button"
          variant="ghost"
        >
          View on GitHub ↗
        </Button>
      </div>
    </div>
  );
}

export { UpdatesSettings };
