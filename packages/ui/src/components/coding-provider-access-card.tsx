import type { CodingProvider } from "@touchgrass/contracts";

import {
  codingProviderAccessStates,
  type CodingProviderAccessState,
} from "../lib/coding-provider-access";
import { Button } from "./button";
import { ProviderConnectionCard } from "./provider-connection-card";
import { Switch } from "./switch";

const providerInstallationGuides: Record<CodingProvider, string> = {
  claude: "https://docs.anthropic.com/en/docs/claude-code/getting-started",
  codex: "https://developers.openai.com/codex/cli/",
};

function CodingProviderAccessCard({
  busy = false,
  displayName: label,
  enabled,
  onCheck,
  onEnabledChange,
  provider,
  savingEnabled = false,
  state,
}: {
  busy?: boolean;
  displayName: string;
  enabled?: boolean | undefined;
  onCheck?: (() => void) | undefined;
  onEnabledChange?: ((enabled: boolean) => void) | undefined;
  provider: CodingProvider;
  savingEnabled?: boolean | undefined;
  state: CodingProviderAccessState;
}) {
  const hasExpandedDetail =
    enabled !== false && (state === "needs-access" || state === "not-installed");
  const settingsCardClass =
    enabled === undefined
      ? undefined
      : enabled === false || state === "detected" || state === "unavailable"
        ? "relative h-[108px] pr-14"
        : "relative min-h-[188px] pr-14";
  let copy: string;
  if (enabled === false) {
    copy = `${label} shows as unavailable in the panel. Its quota and usage are excluded from totals.`;
  } else if (state === "unavailable") {
    copy = `TouchGrassBar could not check ${label} on this Mac. It will try again.`;
  } else if (state === "detected") {
    copy = `${label} was detected on this Mac. No credentials or private provider data were read.`;
  } else if (state === "needs-access") {
    copy = `${label} is installed, but TouchGrassBar cannot read its local state yet.`;
  } else {
    copy = `${label} was not found in Applications or your command-line tools.`;
  }
  const statusTone: "attention" | "neutral" | "ready" =
    enabled === false
      ? "neutral"
      : state === "detected"
        ? "ready"
        : state === "needs-access"
          ? "attention"
          : "neutral";
  const checkAction =
    enabled === false || state === "detected" || onCheck === undefined ? null : (
      <Button
        aria-label={busy ? `Checking ${label}` : `Check ${label} again`}
        className={enabled === undefined ? undefined : "-mr-1.5 mb-1"}
        disabled={busy}
        onClick={onCheck}
        size="quiet"
        type="button"
        variant="ghost"
      >
        {busy ? "Checking…" : "Check again"}
      </Button>
    );
  const action =
    enabled === undefined ? (
      (checkAction ?? undefined)
    ) : (
      <>
        <Switch
          aria-label={`Show ${label} and include its quota and usage in totals`}
          checked={enabled}
          className="absolute top-5 right-5"
          disabled={savingEnabled || onEnabledChange === undefined}
          size="sm"
          {...(onEnabledChange === undefined ? {} : { onCheckedChange: onEnabledChange })}
        />
        <span
          className={
            hasExpandedDetail ? "mt-1.5 -mr-9 flex h-5 items-center" : "-mr-9 flex h-5 items-center"
          }
          data-slot={hasExpandedDetail ? "provider-expanded-action" : "provider-action-spacer"}
          {...(checkAction === null ? { "aria-hidden": true } : {})}
        >
          {checkAction}
        </span>
      </>
    );

  return (
    <ProviderConnectionCard
      action={action}
      className={settingsCardClass}
      data-coding-provider-access-state={state}
      data-provider-enabled={enabled}
      description={copy}
      detail={
        enabled === false || state === "unavailable" ? undefined : state === "needs-access" ? (
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
              Install {label}, open it once, then return here so TouchGrassBar can detect it.
            </small>
            <Button asChild className="mt-1" size="link" variant="link">
              <a
                aria-label={`Open the official ${label} installation guide`}
                href={providerInstallationGuides[provider]}
                rel="noreferrer"
                target="_blank"
              >
                Official installation guide
              </a>
            </Button>
          </div>
        ) : undefined
      }
      label={label}
      provider={provider}
      status={
        enabled === false
          ? "Excluded"
          : (codingProviderAccessStates.find(({ key }) => key === state)?.label ?? state)
      }
      statusTone={statusTone}
    />
  );
}

export { CodingProviderAccessCard };
