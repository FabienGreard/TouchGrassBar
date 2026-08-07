import type { UpdateFailure, UpdateState } from "@touchgrass/contracts";
import { Button, DesktopAppIcon } from "@touchgrass/ui";

type UpdateActions = {
  onCheck?: (() => void) | undefined;
  onDefer?: (() => void) | undefined;
  onInstall?: (() => void) | undefined;
  onOpenLatestDmg?: (() => void) | undefined;
  onRetry?: (() => void) | undefined;
};

type UpdateExperienceProps = UpdateActions & {
  state: UpdateState | null;
  surface: "panel" | "settings";
};

const failureCopy: Record<UpdateFailure, string> = {
  download: "The update download did not finish.",
  interrupted: "The update download was interrupted.",
  "low-disk": "There is not enough free disk space for this update.",
  network: "TouchGrassBar could not reach the update service.",
  permission: "macOS did not allow TouchGrassBar to replace this app.",
  replacement:
    "TouchGrassBar could not finish replacing this app. Use the latest DMG to recover.",
  signature:
    "The update signature could not be verified. The update was not installed.",
  unavailable: "The updater is not available in this build.",
};

function OnlineFeaturePause({ paused }: { paused: boolean }) {
  if (!paused) return null;
  return (
    <p
      className="m-0 border-b border-orange-950/15 bg-orange-100/75 px-4 py-2 text-[10px] leading-4 text-orange-950 contrast-more:border-orange-950 contrast-more:bg-white"
      data-slot="online-feature-pause"
      role="status"
    >
      Update required for online features. Local provider data, Settings, and
      recovery remain available.
    </p>
  );
}

function InstallActions({
  onDefer,
  onInstall,
}: Pick<UpdateActions, "onDefer" | "onInstall">) {
  return (
    <div className="flex items-center justify-end gap-2">
      <Button
        className="motion-reduce:transition-none"
        disabled={onDefer === undefined}
        onClick={onDefer}
        type="button"
        variant="ghost"
      >
        Later
      </Button>
      <Button
        className="motion-reduce:transition-none contrast-more:border-pearl-ink"
        disabled={onInstall === undefined}
        onClick={onInstall}
        type="button"
      >
        Install &amp; Relaunch
      </Button>
    </div>
  );
}

function AvailableUpdate({
  onCheck,
  onDefer,
  onInstall,
  presentation,
  surface,
  version,
}: Pick<UpdateActions, "onCheck" | "onDefer" | "onInstall"> & {
  presentation: "row" | "sheet";
  surface: "panel" | "settings";
  version: string;
}) {
  const row = presentation === "row" && surface === "panel";
  if (row) {
    return (
      <section
        aria-label={`TouchGrassBar ${version} update available`}
        className="flex items-center justify-between gap-3 border-b border-pearl-line bg-green-50/75 px-4 py-2.5 contrast-more:border-pearl-ink contrast-more:bg-white"
        data-slot="update-row"
      >
        <span className="min-w-0">
          <strong className="block truncate text-[10px]">
            Fresh grass available · {version}
          </strong>
          <small className="block text-[8px] text-pearl-muted">
            Signature checked before install.
          </small>
        </span>
        <Button
          aria-label={`Install TouchGrassBar ${version} and relaunch`}
          className="motion-reduce:transition-none contrast-more:border-pearl-ink"
          disabled={onInstall === undefined}
          onClick={onInstall}
          type="button"
        >
          Install &amp; Relaunch
        </Button>
      </section>
    );
  }

  return (
    <section
      aria-labelledby={`update-title-${surface}`}
      className={
        surface === "panel"
          ? "border-b border-pearl-line bg-pearl px-5 py-4 contrast-more:border-pearl-ink"
          : "rounded-[14px] border border-sheet-line bg-white/55 px-5 py-5 shadow-surface contrast-more:border-sheet-ink contrast-more:bg-white"
      }
      data-slot="update-sheet"
      role="status"
    >
      <div className="flex gap-3.5">
        <DesktopAppIcon aria-hidden="true" size="large" />
        <div className="min-w-0 flex-1">
          <h3
            className="m-0 text-[17px] tracking-[-0.025em]"
            id={`update-title-${surface}`}
          >
            Fresh grass available.
          </h3>
          <p className="mt-1.5 mb-2 text-[10px] leading-4 text-pearl-muted">
            TouchGrassBar {version} is ready. Install it now and the menu bar
            app will restart when finished.
          </p>
          <small className="font-mono text-[8px] text-positive">
            Signature checked before install
          </small>
        </div>
      </div>
      <div className="mt-4 flex items-center justify-between gap-3">
        {surface === "settings" ? (
          <Button
            className="motion-reduce:transition-none"
            disabled={onCheck === undefined}
            onClick={onCheck}
            type="button"
            variant="ghost"
          >
            Check for Updates
          </Button>
        ) : (
          <span />
        )}
        <InstallActions onDefer={onDefer} onInstall={onInstall} />
      </div>
    </section>
  );
}

function FailedUpdate({
  failure,
  onOpenLatestDmg,
  onRetry,
}: Pick<UpdateActions, "onOpenLatestDmg" | "onRetry"> & {
  failure: UpdateFailure;
}) {
  return (
    <section
      className="border-b border-orange-950/15 bg-orange-50/90 px-4 py-3 contrast-more:border-orange-950 contrast-more:bg-white"
      data-slot="update-error"
      role="alert"
    >
      <strong className="block text-[11px]">Update not installed</strong>
      <p className="mt-1 mb-3 text-[9px] leading-4 text-orange-950">
        {failureCopy[failure]}
      </p>
      <div className="flex flex-wrap gap-2">
        <Button
          className="motion-reduce:transition-none"
          disabled={onRetry === undefined}
          onClick={onRetry}
          type="button"
        >
          Retry
        </Button>
        <Button
          className="motion-reduce:transition-none"
          disabled={onOpenLatestDmg === undefined}
          onClick={onOpenLatestDmg}
          type="button"
          variant="secondary"
        >
          Download latest DMG
        </Button>
      </div>
    </section>
  );
}

function UpdateExperience({
  onCheck,
  onDefer,
  onInstall,
  onOpenLatestDmg,
  onRetry,
  state,
  surface,
}: UpdateExperienceProps) {
  if (state === null) {
    return surface === "settings" ? (
      <p className="m-0 text-[10px] text-sheet-muted">Updater unavailable.</p>
    ) : null;
  }

  const update = state.update;
  const status = update.status;
  if (status === "available") {
    return (
      <>
        <OnlineFeaturePause paused={state.onlineFeaturesPaused} />
        <AvailableUpdate
          onCheck={onCheck}
          onDefer={onDefer}
          onInstall={onInstall}
          presentation={update.presentation}
          surface={surface}
          version={update.version}
        />
      </>
    );
  }
  if (status === "failed") {
    return (
      <>
        <OnlineFeaturePause paused={state.onlineFeaturesPaused} />
        <FailedUpdate
          failure={update.failure}
          onOpenLatestDmg={onOpenLatestDmg}
          onRetry={onRetry}
        />
      </>
    );
  }
  if (surface === "panel") {
    if (status === "downloading" || status === "installing") {
      return (
        <section
          aria-live="polite"
          className="border-b border-pearl-line bg-green-50/75 px-4 py-2.5 text-[10px] contrast-more:border-pearl-ink contrast-more:bg-white"
          data-slot="update-progress"
        >
          {status === "downloading"
            ? `Downloading signed update${update.progressPercent == null ? "…" : ` · ${update.progressPercent}%`}`
            : "Update verified. Installing and relaunching…"}
        </section>
      );
    }
    return null;
  }

  return (
    <div className="grid gap-3" data-slot="update-settings-state">
      <div className="flex items-center justify-between gap-6 rounded-[12px] bg-white/38 px-4 py-3.5 contrast-more:outline contrast-more:outline-1 contrast-more:outline-sheet-ink">
        <span>
          <strong className="block text-[12px]">
            Version {state.currentVersion}
          </strong>
          <small className="mt-0.5 block text-[9px] text-sheet-muted">
            {status === "checking"
              ? "Checking the stable channel…"
              : status === "upToDate"
                ? "TouchGrassBar is up to date."
                : status === "unavailable"
                  ? "Updater unavailable in this build."
                  : status === "downloading"
                    ? "Downloading and verifying the update…"
                    : status === "installing"
                      ? "Installing and relaunching…"
                      : "Stable channel"}
          </small>
        </span>
        <Button
          className="motion-reduce:transition-none"
          disabled={
            onCheck === undefined ||
            status === "unavailable" ||
            status === "checking" ||
            status === "downloading" ||
            status === "installing"
          }
          onClick={onCheck}
          type="button"
          variant="ghost"
        >
          Check for Updates
        </Button>
      </div>
      <p className="m-0 px-4 text-[9px] leading-4 text-sheet-muted">
        TouchGrassBar checks quietly when the panel first opens, at most once
        every 24 hours. It never installs or restarts without your approval.
      </p>
    </div>
  );
}

export { UpdateExperience };
export type { UpdateActions, UpdateExperienceProps };
