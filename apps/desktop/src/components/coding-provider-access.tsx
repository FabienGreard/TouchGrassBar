import { Button, ProviderConnectionCard } from "@touchgrass/ui";

import {
  codingProviderAccessStates,
  type CodingProviderAccessState,
} from "@/components/coding-provider-access-state";

function CodingProviderAccessCard({
  busy = false,
  onCheck,
  onViewInstallationSteps,
  provider,
  state,
}: {
  busy?: boolean;
  onCheck?: (() => void) | undefined;
  onViewInstallationSteps?: (() => void) | undefined;
  provider: "claude" | "codex";
  state: CodingProviderAccessState;
}) {
  const label = provider === "codex" ? "Codex" : "Claude";
  let copy: string;
  if (state === "unavailable") {
    copy = "Provider detection is not connected in this build.";
  } else if (state === "detected") {
    copy = `${label} was detected on this Mac. No credentials or private provider data were read.`;
  } else if (state === "ready") {
    copy = "Detected locally and reporting provider limits.";
  } else if (state === "needs-access") {
    copy = `${label} is installed, but TouchGrassBar cannot read its local state yet.`;
  } else {
    copy = `${label} was not found in Applications or your command-line tools.`;
  }
  const statusTone: "attention" | "neutral" | "ready" =
    state === "ready" || state === "detected"
      ? "ready"
      : state === "needs-access"
        ? "attention"
        : "neutral";

  return (
    <ProviderConnectionCard
      action={
        state === "unavailable" || onCheck === undefined ? undefined : (
          <Button
            aria-label={
              busy
                ? `Checking ${label}`
                : state === "ready"
                  ? `Check ${label} now`
                  : `Check ${label} again`
            }
            disabled={busy}
            onClick={onCheck}
            size="quiet"
            type="button"
            variant="ghost"
          >
            {busy
              ? "Checking…"
              : state === "ready"
                ? "Check now"
                : "Check again"}
          </Button>
        )
      }
      data-coding-provider-access-state={state}
      description={copy}
      detail={
        state === "unavailable" ? undefined : state === "needs-access" ? (
          <div className="mt-3 rounded-[9px] border border-[#e3d1a6] bg-[#fff8e8] px-3 py-2.5">
            <strong className="block text-[10px]">Finish local access</strong>
            <small className="mt-1 block text-[9px] leading-4 text-[#6d5a32]">
              Open {label} once and finish its local setup, then return here.
            </small>
          </div>
        ) : state === "not-installed" ? (
          <div className="mt-3 rounded-[9px] border border-sheet-line bg-[#20263d06] px-3 py-2.5">
            <strong className="block text-[10px]">Connect {label}</strong>
            <small className="mt-1 block text-[9px] leading-4 text-sheet-muted">
              Install {label}, open it once, then return here so TouchGrassBar
              can detect it.
            </small>
            {onViewInstallationSteps ? (
              <Button
                className="mt-2"
                onClick={onViewInstallationSteps}
                size="link"
                type="button"
                variant="link"
              >
                View installation steps
              </Button>
            ) : null}
          </div>
        ) : undefined
      }
      label={label}
      provider={provider}
      status={
        codingProviderAccessStates.find(({ key }) => key === state)?.label ??
        state
      }
      statusTone={statusTone}
    />
  );
}

export { CodingProviderAccessCard };
