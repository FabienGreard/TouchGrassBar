import {
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

import { useCopyText } from "@/components/use-copy-text";
import { defaultDoomerboardQuery, type DoomerboardQuery } from "@/native-state/doomerboard-query";

const emptyProviders: readonly DoomerboardProvider[] = [];

const doomerboardSkeletonCards = [
  {
    card: "min-h-[112px] border-rank-silver-border bg-rank-silver",
    key: "second",
    medal: "h-[29px] w-[29px] bg-rank-silver text-[12px]",
    rank: 2,
  },
  {
    card: "min-h-[136px] border-rank-gold-border bg-rank-gold",
    key: "first",
    medal: "h-[33px] w-[33px] bg-rank-gold text-[15px]",
    rank: 1,
  },
  {
    card: "min-h-[112px] border-rank-bronze-border bg-rank-bronze",
    key: "third",
    medal: "h-[29px] w-[29px] bg-rank-bronze text-[12px]",
    rank: 3,
  },
] as const;

function DoomerboardSkeleton() {
  return (
    <div
      aria-busy="true"
      aria-label="Loading Doomerboard"
      className="h-full"
      data-slot="doomerboard-loading"
      role="status"
    >
      <span className="sr-only">Loading Doomerboard scores…</span>
      <div
        aria-hidden="true"
        className="pointer-events-none grid h-full animate-pulse grid-cols-[1fr_1.12fr_1fr] items-end gap-[5px] px-3.5 pt-[25px] pb-[11px] motion-reduce:animate-none"
        inert
      >
        {doomerboardSkeletonCards.map((style) => (
          <div
            className={`relative flex flex-col items-center rounded-t-[13px] rounded-b-[8px] border px-1 py-2 text-center shadow-rank-card backdrop-blur-[8px] ${style.card}`}
            data-doomerboard-skeleton-rank={style.rank}
            key={style.key}
          >
            <span
              className={`absolute -top-3.5 grid place-items-center rounded-full font-extrabold text-pearl-ink/20 shadow-control contrast-more:text-pearl-ink/40 ${style.medal}`}
              data-slot="doomerboard-skeleton-medal"
            >
              {style.rank}
            </span>
            <span className="mt-[18px] h-1.5 w-12 rounded-full bg-pearl-ink/10 contrast-more:bg-pearl-ink/25" />
            <span className="mt-auto h-2.5 w-16 rounded-full bg-pearl-ink/10 contrast-more:bg-pearl-ink/25" />
            <span className="mt-1.5 h-1.5 w-12 rounded-full bg-pearl-ink/10 contrast-more:bg-pearl-ink/25" />
            <span className="mt-3 h-3 w-14 rounded-full bg-pearl-ink/10 contrast-more:bg-pearl-ink/25" />
            <span className="mt-1.5 h-1.5 w-10 rounded-full bg-pearl-ink/10 contrast-more:bg-pearl-ink/25" />
          </div>
        ))}
      </div>
    </div>
  );
}

function DoomerboardUnavailable({
  selectionUnavailable = false,
}: {
  selectionUnavailable?: boolean;
}) {
  return (
    <div
      aria-label="Leaderboard unavailable"
      className="mx-3.5 flex h-full flex-col items-center justify-center rounded-[12px] border border-dashed border-pearl-line bg-pearl-surface px-6 py-3.5 text-center shadow-surface contrast-more:border-pearl-ink"
    >
      <RankingIcon aria-hidden="true" size={20} />
      <strong className="mt-1.5 text-[10px]">Leaderboard unavailable</strong>
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
      <strong className="mt-1.5 text-[10px]">Your Leaderboard is lonely</strong>
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
  loading = false,
  onAddTokenmaxxer = () => undefined,
  onSelectionChange = () => undefined,
  providers = emptyProviders,
  rows,
  selection = defaultDoomerboardQuery,
  tokenmaxxerRows,
}: {
  currentProfile?: DoomerboardCurrentProfile | null | undefined;
  loading?: boolean | undefined;
  onAddTokenmaxxer?: (() => void) | undefined;
  onSelectionChange?: ((selection: DoomerboardQuery) => void) | undefined;
  providers?: readonly DoomerboardProvider[] | undefined;
  rows?: readonly DoomerboardRow[] | undefined;
  selection?: DoomerboardQuery | undefined;
  tokenmaxxerRows?: readonly DoomerboardRow[] | undefined;
}) {
  const currentProfileText = currentProfile ? currentProfile.touchGrassId.replace(/^#/, "") : "";
  const { copyStatus, copyText } = useCopyText(currentProfileText);
  const selectedRows = selection.audience === "global" ? rows : tokenmaxxerRows;
  const rowsEmpty = selectedRows !== undefined && selectedRows.length === 0;
  const period =
    selection.windowDays === 1 ? "today" : selection.windowDays === 7 ? "week" : "month";
  const updateAudience = (audience: DoomerboardAudience) =>
    onSelectionChange({ ...selection, audience });
  const updatePeriod = (nextPeriod: string) => {
    const windowDays =
      nextPeriod === "today" ? 1 : nextPeriod === "week" ? 7 : nextPeriod === "month" ? 30 : null;
    if (windowDays === null) return;
    onSelectionChange({ ...selection, windowDays });
  };
  const updateProvider = (scope: string) => {
    if (scope !== "claude" && scope !== "codex" && scope !== "combined") {
      return;
    }
    onSelectionChange({ ...selection, scope });
  };
  return (
    <section
      aria-label={
        loading
          ? "Loading Doomerboard"
          : selection.audience === "mine"
            ? rowsEmpty
              ? "My Tokenmaxxers empty"
              : selectedRows !== undefined
                ? "My Tokenmaxxers rankings"
                : "Leaderboard unavailable"
            : rowsEmpty
              ? "Leaderboard unavailable"
              : selectedRows !== undefined
                ? "Leaderboard rankings"
                : "Leaderboard unavailable"
      }
      className="pb-2"
    >
      <DoomerboardToolbar
        audience={selection.audience}
        copyStatus={copyStatus}
        currentProfile={currentProfile}
        onAudienceChange={updateAudience}
        onCopyCurrentProfile={currentProfile ? () => void copyText() : undefined}
        onPeriodChange={updatePeriod}
        onProviderChange={updateProvider}
        period={period}
        provider={selection.scope}
        providers={providers}
      />
      <div className="mt-3 h-[180px]" data-slot="doomerboard-viewport">
        {loading ? (
          <DoomerboardSkeleton />
        ) : selection.audience === "mine" && rowsEmpty ? (
          <TokenmaxxersEmpty onAddTokenmaxxer={onAddTokenmaxxer} />
        ) : selection.audience === "global" && rowsEmpty ? (
          <DoomerboardUnavailable />
        ) : selectedRows !== undefined ? (
          <DoomerboardRankings rows={selectedRows} />
        ) : (
          <DoomerboardUnavailable
            selectionUnavailable={rows !== undefined || tokenmaxxerRows !== undefined}
          />
        )}
      </div>
    </section>
  );
}

export { Doomerboard, TokenmaxxersEmpty };
export type { DoomerboardCurrentProfile as CurrentProfile };
export type { DoomerboardRow } from "@touchgrass/ui";
