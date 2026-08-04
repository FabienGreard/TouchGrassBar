import { Button, ProviderConnectionCard } from "@touchgrass/ui";

import {
  codingProviderAccessStates,
  type CodingProviderAccessState,
} from "@/components/coding-provider-access-state";

function CodingProviderAccessCard({
  provider,
  state,
}: {
  provider: "claude" | "codex";
  state: CodingProviderAccessState;
}) {
  const label = provider === "codex" ? "Codex" : "Claude";
  const copy =
    state === "unavailable"
      ? "Provider detection is not connected in this build."
      : state === "ready"
        ? "Detected locally and reporting provider limits."
        : state === "needs-access"
          ? `${label} is installed, but TouchGrassBar cannot read its local state yet.`
          : `${label} was not found in Applications or your command-line tools.`;
  const statusTone: "attention" | "neutral" | "ready" =
    state === "ready"
      ? "ready"
      : state === "needs-access"
        ? "attention"
        : "neutral";

  return (
    <ProviderConnectionCard
      action={
        state === "unavailable" ? undefined : (
          <Button size="quiet" type="button" variant="ghost">
            {state === "ready" ? "Check now" : "Check again"}
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
            <Button className="mt-2" size="link" type="button" variant="link">
              View installation steps
            </Button>
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
