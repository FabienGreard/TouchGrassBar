import { useRef, useState } from "react";

import {
  Button,
  DoomerboardRankings,
  DoomerboardToolbar,
  InviteIcon,
  EllipsisIcon,
  PanelMenu,
  PanelMenuContent,
  PanelMenuItem,
  PanelMenuTrigger,
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

function FriendActions({
  onRemove,
  row,
}: {
  onRemove: (touchGrassId: string) => Promise<boolean>;
  row: DoomerboardRow;
}) {
  const inFlight = useRef(false);
  const [open, setOpen] = useState(false);
  const [status, setStatus] = useState<"idle" | "removing" | "failed">("idle");
  return (
    <PanelMenu onOpenChange={setOpen} open={open}>
      <PanelMenuTrigger asChild>
        <button
          aria-label={`Friend actions for ${row.displayName}`}
          className="grid size-5 cursor-pointer place-items-center rounded text-pearl-muted hover:bg-pearl-ink/5 hover:text-pearl-ink focus-visible:outline-2 focus-visible:outline-pearl-ink"
          title={`Friend actions for ${row.displayName}`}
          type="button"
        >
          <EllipsisIcon aria-hidden="true" size={14} />
        </button>
      </PanelMenuTrigger>
      <PanelMenuContent align="end" sideOffset={4}>
        <PanelMenuItem
          disabled={status === "removing"}
          onSelect={(event) => {
            event.preventDefault();
            if (inFlight.current) return;
            inFlight.current = true;
            setStatus("removing");
            void onRemove(row.touchGrassId.replace(/^#/, ""))
              .catch(() => false)
              .then((removed) => {
                inFlight.current = false;
                setStatus(removed ? "idle" : "failed");
                if (removed) setOpen(false);
              });
          }}
        >
          {status === "removing" ? "Removing…" : "Remove friend"}
        </PanelMenuItem>
        {status === "failed" ? (
          <small className="px-2 pb-1 text-[9px] text-pearl-muted" role="alert">
            Could not remove friend. Try again.
          </small>
        ) : null}
      </PanelMenuContent>
    </PanelMenu>
  );
}

function LeaderboardId({ touchGrassId }: { touchGrassId: string }) {
  const canonicalId = touchGrassId.replace(/^#/, "");
  const { copyStatus, copyText } = useCopyText(canonicalId);
  const feedback =
    copyStatus === "copied" ? "Copied" : copyStatus === "unavailable" ? "Unavailable" : "";

  return (
    <button
      aria-label={`Copy TouchGrass ID ${canonicalId}`}
      className="relative cursor-pointer rounded-sm hover:text-pearl-ink hover:underline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-pearl-ink"
      data-copy-status={copyStatus}
      onClick={() => void copyText()}
      title={copyStatus === "unavailable" ? "Copy unavailable" : feedback || "Copy TouchGrass ID"}
      type="button"
    >
      <span className={copyStatus === "idle" ? undefined : "invisible"}>{touchGrassId}</span>
      <span
        aria-live="polite"
        className="absolute inset-0 text-pearl-ink"
        data-copy-feedback={copyStatus}
      >
        {feedback}
      </span>
    </button>
  );
}

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
      aria-label="My friends empty"
      className="mx-3.5 flex h-full flex-col items-center justify-center rounded-[12px] border border-dashed border-pearl-line bg-pearl-surface px-6 py-3.5 text-center shadow-surface contrast-more:border-pearl-ink"
    >
      <InviteIcon aria-hidden="true" size={20} />
      <strong className="mt-1.5 text-[10px]">Your Leaderboard is lonely</strong>
      <small className="mt-0.5 max-w-[260px] text-[8px] leading-3.5 text-pearl-muted contrast-more:text-pearl-ink">
        Add friends by TouchGrass ID to compare scores.
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
  onRemoveFriend,
  onSelectionChange = () => undefined,
  onSelectionIntent = () => undefined,
  providers = emptyProviders,
  rows,
  selection = defaultDoomerboardQuery,
  tokenmaxxerRows,
}: {
  currentProfile?: DoomerboardCurrentProfile | null | undefined;
  loading?: boolean | undefined;
  onAddTokenmaxxer?: (() => void) | undefined;
  onRemoveFriend?: ((touchGrassId: string) => Promise<boolean>) | undefined;
  onSelectionChange?: ((selection: DoomerboardQuery) => void) | undefined;
  onSelectionIntent?: ((selection: DoomerboardQuery) => void) | undefined;
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
              ? "My friends empty"
              : selectedRows !== undefined
                ? "My friends rankings"
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
        onAddFriend={
          selection.audience === "mine" && selectedRows !== undefined && !rowsEmpty
            ? onAddTokenmaxxer
            : undefined
        }
        onAudienceChange={updateAudience}
        onCopyCurrentProfile={currentProfile ? () => void copyText() : undefined}
        onPeriodChange={updatePeriod}
        onProviderChange={updateProvider}
        onSelectionIntent={({ audience, period: nextPeriod, provider: nextProvider }) => {
          const windowDays =
            nextPeriod === "today"
              ? 1
              : nextPeriod === "week"
                ? 7
                : nextPeriod === "month"
                  ? 30
                  : null;
          if (
            windowDays === null ||
            (nextProvider !== "claude" && nextProvider !== "codex" && nextProvider !== "combined")
          ) {
            return;
          }
          onSelectionIntent({ audience, scope: nextProvider, windowDays });
        }}
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
          <DoomerboardRankings
            renderRowAction={
              selection.audience === "mine" && currentProfile && onRemoveFriend
                ? (row) =>
                    row.touchGrassId.replace(/^#/, "") === currentProfileText ? null : (
                      <FriendActions
                        key={`${currentProfileText}:${row.touchGrassId}`}
                        onRemove={onRemoveFriend}
                        row={row}
                      />
                    )
                : undefined
            }
            renderTouchGrassId={(row) => <LeaderboardId touchGrassId={row.touchGrassId} />}
            rows={selectedRows}
          />
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
