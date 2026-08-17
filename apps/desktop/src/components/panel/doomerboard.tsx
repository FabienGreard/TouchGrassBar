import {
  ArrowExpand01Icon,
  ArrowShrink02Icon,
  Button,
  DoomerboardRankings,
  DoomerboardToolbar,
  InviteIcon,
  RankingIcon,
} from "@touchgrass/ui";
import type {
  DoomerboardAudience,
  DoomerboardCurrentProfile,
  DoomerboardProvider,
  DoomerboardRow,
} from "@touchgrass/ui";
import { useState } from "react";

import { useCopyText } from "@/components/use-copy-text";

const emptyProviders: readonly DoomerboardProvider[] = [];

function DoomerboardUnavailable({
  selectionUnavailable = false,
}: {
  selectionUnavailable?: boolean;
}) {
  return (
    <div
      aria-label="Doomerboard unavailable"
      className="mx-3.5 flex h-full flex-col items-center justify-center rounded-[12px] border border-dashed border-pearl-line bg-pearl-surface px-6 py-3.5 text-center shadow-surface contrast-more:border-pearl-ink"
    >
      <RankingIcon aria-hidden="true" size={20} />
      <strong className="mt-1.5 text-[10px]">Doomerboard unavailable</strong>
      <small className="mt-0.5 max-w-[260px] text-[8px] leading-3.5 text-pearl-muted contrast-more:text-pearl-ink">
        {selectionUnavailable
          ? "Scores are unavailable for this selection."
          : "Profile and synchronized scores are not ready."}
      </small>
    </div>
  );
}

function TokenmaxxersEmpty({
  onAddTokenmaxxer = () => undefined,
}: {
  onAddTokenmaxxer?: (() => void) | undefined;
}) {
  return (
    <div
      aria-label="My Tokenmaxxers empty"
      className="mx-3.5 flex h-full flex-col items-center justify-center rounded-[12px] border border-dashed border-pearl-line bg-pearl-surface px-6 py-3.5 text-center shadow-surface contrast-more:border-pearl-ink"
    >
      <InviteIcon aria-hidden="true" size={20} />
      <strong className="mt-1.5 text-[10px]">Your Doomerboard is lonely</strong>
      <small className="mt-0.5 max-w-[260px] text-[8px] leading-3.5 text-pearl-muted contrast-more:text-pearl-ink">
        Add Tokenmaxxers by TouchGrass ID to compare scores.
      </small>
      <div className="mt-2">
        <Button onClick={onAddTokenmaxxer} type="button">
          Add a Tokenmaxxer
        </Button>
      </div>
    </div>
  );
}

function Doomerboard({
  currentProfile = null,
  expanded = false,
  initialAudience = "global",
  onAddTokenmaxxer = () => undefined,
  onExpandedChange = () => undefined,
  providers = emptyProviders,
  rows,
  tokenmaxxerRows,
}: {
  currentProfile?: DoomerboardCurrentProfile | null | undefined;
  expanded?: boolean | undefined;
  initialAudience?: DoomerboardAudience | undefined;
  onAddTokenmaxxer?: (() => void) | undefined;
  onExpandedChange?: ((expanded: boolean) => void) | undefined;
  providers?: readonly DoomerboardProvider[] | undefined;
  rows?: readonly DoomerboardRow[] | undefined;
  tokenmaxxerRows?: readonly DoomerboardRow[] | undefined;
}) {
  const [audience, setAudience] =
    useState<DoomerboardAudience>(initialAudience);
  const [period, setPeriod] = useState("today");
  const [provider, setProvider] = useState("combined");
  const currentProfileText = currentProfile
    ? `${currentProfile.displayName}${currentProfile.touchGrassId}`
    : "";
  const { copyStatus, copyText } = useCopyText(currentProfileText);
  const globalRowsMatchSelection =
    rows !== undefined &&
    audience === "global" &&
    period === "today" &&
    provider === "combined";
  const tokenmaxxersMatchSelection =
    tokenmaxxerRows !== undefined &&
    audience === "mine" &&
    period === "today" &&
    provider === "combined";
  const tokenmaxxersEmpty =
    audience === "mine" && tokenmaxxerRows === undefined;
  return (
    <section
      aria-label={
        tokenmaxxersMatchSelection
          ? "My Tokenmaxxers rankings"
          : tokenmaxxersEmpty
            ? "My Tokenmaxxers empty"
            : globalRowsMatchSelection
              ? "Doomerboard rankings"
              : "Doomerboard unavailable"
      }
      className="pb-2"
      data-expanded={expanded}
    >
      <DoomerboardToolbar
        action={
          <Button
            aria-label={expanded ? "Collapse Doomerboard" : "Expand Doomerboard"}
            onClick={() => onExpandedChange(!expanded)}
            size="quiet"
            title={expanded ? "Collapse Doomerboard" : "Expand Doomerboard"}
            type="button"
            variant="ghost"
          >
            {expanded ? (
              <ArrowShrink02Icon aria-hidden="true" size={13} />
            ) : (
              <ArrowExpand01Icon aria-hidden="true" size={13} />
            )}
          </Button>
        }
        audience={audience}
        copyStatus={copyStatus}
        currentProfile={currentProfile}
        onAudienceChange={setAudience}
        onCopyCurrentProfile={
          currentProfile ? () => void copyText() : undefined
        }
        onPeriodChange={setPeriod}
        onProviderChange={setProvider}
        period={period}
        provider={provider}
        providers={providers}
      />
      <div
        className={expanded ? "mt-3 h-[500px]" : "mt-3 h-[180px]"}
        data-slot="doomerboard-viewport"
      >
        {tokenmaxxersMatchSelection ? (
          <DoomerboardRankings
            rows={tokenmaxxerRows}
            variant={expanded ? "expanded" : "compact"}
          />
        ) : tokenmaxxersEmpty ? (
          <TokenmaxxersEmpty onAddTokenmaxxer={onAddTokenmaxxer} />
        ) : globalRowsMatchSelection ? (
          <DoomerboardRankings
            rows={rows}
            variant={expanded ? "expanded" : "compact"}
          />
        ) : (
          <DoomerboardUnavailable selectionUnavailable={rows !== undefined} />
        )}
      </div>
    </section>
  );
}

export { Doomerboard, TokenmaxxersEmpty };
export type { DoomerboardCurrentProfile as CurrentProfile };
export type { DoomerboardRow } from "@touchgrass/ui";
