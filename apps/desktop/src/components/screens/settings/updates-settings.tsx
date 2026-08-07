import { Button } from "@touchgrass/ui";
import type { UpdateState } from "@touchgrass/contracts";

import { UpdateExperience, type UpdateActions } from "@/update/update-experience";

function UpdatesSettings({
  onCheck,
  onDefer,
  onInstall,
  onOpenLatestDmg,
  onCheckForUpdates,
  onOpenSource,
  onRetry,
  state,
}: UpdateActions & {
  onCheckForUpdates?: (() => void) | undefined;
  onOpenSource?: (() => void) | undefined;
  state: UpdateState | null;
}) {
  return (
    <div className="grid gap-3" data-slot="updates-settings">
      <UpdateExperience
        onCheck={onCheck ?? onCheckForUpdates}
        onDefer={onDefer}
        onInstall={onInstall}
        onOpenLatestDmg={onOpenLatestDmg}
        onRetry={onRetry}
        state={state}
        surface="settings"
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
