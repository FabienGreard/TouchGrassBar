import type { ComponentProps } from "react";

import { cn } from "#lib/utils";
import { ScrollArea } from "./scroll-area";

type DoomerboardRow = {
  displayName: string;
  note?: string;
  rank: number;
  tokenScore: string;
  touchGrassId: string;
};

type DoomerboardRankingsProps = Omit<ComponentProps<typeof ScrollArea>, "children"> & {
  ledgerLimit?: number | undefined;
  rows: readonly DoomerboardRow[];
};

const rankStyles = {
  1: {
    card: "min-h-[136px] border-rank-gold-border bg-rank-gold",
    medal: "h-[33px] w-[33px] bg-rank-gold text-[15px]",
  },
  2: {
    card: "min-h-[112px] border-rank-silver-border bg-rank-silver",
    medal: "h-[29px] w-[29px] bg-rank-silver text-[12px]",
  },
  3: {
    card: "min-h-[112px] border-rank-bronze-border bg-rank-bronze",
    medal: "h-[29px] w-[29px] bg-rank-bronze text-[12px]",
  },
} as const;

function DoomerboardRankings({
  className,
  ledgerLimit,
  rows,
  viewportClassName,
  ...props
}: DoomerboardRankingsProps) {
  const podiumOrder = new Map([
    [2, 0],
    [1, 1],
    [3, 2],
  ]);
  const podium = rows
    .filter((row) => row.rank >= 1 && row.rank <= 3)
    .sort(
      (a, b) =>
        (podiumOrder.get(a.rank) ?? a.rank) -
          (podiumOrder.get(b.rank) ?? b.rank) ||
        a.touchGrassId.localeCompare(b.touchGrassId),
    );
  const allLedgerRows = rows
    .filter((row) => row.rank > 3)
    .sort(
      (a, b) =>
        a.rank - b.rank || a.touchGrassId.localeCompare(b.touchGrassId),
    );
  const ledger =
    ledgerLimit === undefined
      ? allLedgerRows
      : allLedgerRows.slice(0, ledgerLimit);

  return (
    <ScrollArea
      aria-label="Leaderboard rankings"
      className={cn("h-full", className)}
      data-doomerboard-scroll=""
      viewportClassName={cn(
        "select-none overscroll-contain",
        viewportClassName,
      )}
      {...props}
    >
      <div
        className={cn(
          "grid items-end gap-[5px] px-3.5 pt-[25px] pb-[11px]",
          podium.length === 3 && "grid-cols-[1fr_1.12fr_1fr]",
        )}
        style={
          podium.length === 3
            ? undefined
            : {
                gridTemplateColumns: `repeat(${Math.max(podium.length, 1)}, minmax(0, 1fr))`,
              }
        }
      >
        {podium.map((row) => {
          const style = rankStyles[row.rank as 1 | 2 | 3];
          return (
            <article
              className={`relative flex flex-col items-center rounded-t-[13px] rounded-b-[8px] border px-1 py-2 text-center shadow-rank-card backdrop-blur-[8px] ${style.card}`}
              key={row.touchGrassId}
            >
              <div
                className={`absolute -top-3.5 grid place-items-center rounded-full font-extrabold shadow-control ${style.medal}`}
              >
                {row.rank}
              </div>
              <span className="mt-[15px] font-mono text-[7px] tracking-[0.08em] text-pearl-muted">
                {row.note}
              </span>
              <b className="mt-auto text-[12px]">{row.displayName}</b>
              <small className="mt-0.5 text-[7px] text-pearl-muted">
                {row.touchGrassId}
              </small>
              <strong className="mt-[7px] text-[16px]">
                {row.tokenScore}
              </strong>
            </article>
          );
        })}
      </div>
      {ledger.length > 0 ? (
        <div
          aria-label="More Leaderboard ranks"
          className="mx-3.5 border-t border-pearl-line"
          data-slot="doomerboard-ledger"
        >
          {ledger.map((row) => (
            <article
              className="grid grid-cols-[30px_1fr_70px] items-center border-b border-pearl-line px-2.5 py-[8px] text-pearl-ink last:border-b-0"
              key={row.touchGrassId}
            >
              <strong className="text-pearl-muted">{row.rank}</strong>
              <span>
                <b className="block text-[11px]">{row.displayName}</b>
                <small className="mt-0.5 block text-[7px] text-pearl-muted">
                  {row.touchGrassId}
                </small>
              </span>
              <b className="text-right text-[12px]">{row.tokenScore}</b>
            </article>
          ))}
        </div>
      ) : null}
    </ScrollArea>
  );
}

export { DoomerboardRankings };
export type { DoomerboardRankingsProps, DoomerboardRow };
