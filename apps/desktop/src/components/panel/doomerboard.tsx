import {
  Button,
  InviteIcon,
  PanelMenu,
  PanelMenuContent,
  PanelMenuRadioGroup,
  PanelMenuRadioItem,
  PanelMenuTrigger,
  RankingIcon,
  ScrollArea,
  SegmentedControl,
  SegmentedControlItem,
} from "@touchgrass/ui";
import type { CodingProvider } from "@touchgrass/contracts";
import { useState } from "react";

import { useCopyText } from "@/components/use-copy-text";

type DoomerboardRow = {
  id: string;
  name: string;
  note?: string;
  rank: number;
  score: string;
};

const periodOptions = [
  { label: "Today", value: "today" },
  { label: "7 days", value: "week" },
  { label: "30 days", value: "month" },
] as const;

type QueryOption = { label: string; value: string };
type DoomerboardProvider = {
  displayName: string;
  provider: CodingProvider;
};

type QuerySelectorProps = {
  label: string;
  onValueChange: (value: string) => void;
  options: readonly QueryOption[];
  value: string;
};

function QuerySelector({
  label,
  onValueChange,
  options,
  value,
}: QuerySelectorProps) {
  const selectedLabel =
    options.find((option) => option.value === value)?.label ?? value;

  return (
    <PanelMenu>
      <PanelMenuTrigger asChild>
        <Button
          aria-label={`Select Doomerboard ${label}`}
          size="quiet"
          type="button"
          variant="ghost"
        >
          {selectedLabel}
        </Button>
      </PanelMenuTrigger>
      <PanelMenuContent align="end" side="top" sideOffset={9} size="compact">
        <PanelMenuRadioGroup onValueChange={onValueChange} value={value}>
          {options.map((option) => (
            <PanelMenuRadioItem key={option.value} value={option.value}>
              {option.label}
            </PanelMenuRadioItem>
          ))}
        </PanelMenuRadioGroup>
      </PanelMenuContent>
    </PanelMenu>
  );
}

type Audience = "global" | "mine";

type CurrentUser = Pick<DoomerboardRow, "id" | "name">;

type DoomerboardToolbarProps = {
  audience: Audience;
  onAudienceChange: (audience: Audience) => void;
  onPeriodChange: (period: string) => void;
  onProviderChange: (provider: string) => void;
  period: string;
  provider: string;
  providers: readonly DoomerboardProvider[];
  currentUser: CurrentUser | null;
};

function CurrentUserProfile({
  currentUser,
}: {
  currentUser: CurrentUser | null;
}) {
  const profile = currentUser
    ? `${currentUser.name}${currentUser.id}`
    : "";
  const { copyStatus, copyText } = useCopyText(profile);

  if (currentUser === null) {
    return (
      <small
        aria-label="Current user profile unavailable"
        className="font-mono text-[7px] text-pearl-muted"
        data-slot="current-user-profile"
      >
        Profile unavailable
      </small>
    );
  }

  return (
    <span
      className="inline-flex min-w-0 items-center gap-0.5"
      data-slot="current-user-profile"
    >
      <Button
        aria-label={`Copy current user profile ${profile}`}
        className="max-w-[142px] rounded-[5px] font-mono text-[7px] font-medium"
        data-copy-status={copyStatus}
        data-slot="current-user-profile-action"
        onClick={() => void copyText()}
        size="quiet"
        title={
          copyStatus === "copied"
            ? "Copied"
            : copyStatus === "unavailable"
              ? "Copy unavailable"
              : "Copy profile"
        }
        type="button"
        variant="ghost"
      >
        <span className="truncate">{profile}</span>
      </Button>
      <span
        aria-live="polite"
        className="font-mono text-[7px] text-pearl-ink"
        data-copy-feedback={copyStatus}
      >
        {copyStatus === "copied"
          ? "Copied"
          : copyStatus === "unavailable"
            ? "Unavailable"
            : ""}
      </span>
    </span>
  );
}

function DoomerboardToolbar({
  audience,
  onAudienceChange,
  onPeriodChange,
  onProviderChange,
  period,
  provider,
  providers,
  currentUser,
}: DoomerboardToolbarProps) {
  const providerOptions: QueryOption[] = [
    { label: "Combined", value: "combined" },
    ...providers.map(({ displayName, provider: providerId }) => ({
      label: displayName,
      value: providerId,
    })),
  ];

  return (
    <>
      <header className="flex items-center justify-between px-3.5 pt-3">
        <div className="flex min-w-0 items-center gap-1.5">
          <strong className="shrink-0 text-[10px]">Doomerboard</strong>
          <CurrentUserProfile currentUser={currentUser} />
        </div>
        <div className="flex items-center gap-0.5 text-pearl-muted">
          <QuerySelector
            label="period"
            onValueChange={onPeriodChange}
            options={periodOptions}
            value={period}
          />
          <span aria-hidden="true" className="text-[8px]">
            ·
          </span>
          <QuerySelector
            label="provider"
            onValueChange={onProviderChange}
            options={providerOptions}
            value={provider}
          />
        </div>
      </header>
      <div className="mx-3.5 mt-2">
        <SegmentedControl
          aria-label="Doomerboard audience"
          onValueChange={(value) => onAudienceChange(value as Audience)}
          value={audience}
        >
          <SegmentedControlItem value="mine">Friends</SegmentedControlItem>
          <SegmentedControlItem value="global">Global</SegmentedControlItem>
        </SegmentedControl>
      </div>
    </>
  );
}

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
      aria-label="Friends empty"
      className="mx-3.5 flex h-full flex-col items-center justify-center rounded-[12px] border border-dashed border-pearl-line bg-pearl-surface px-6 py-3.5 text-center shadow-surface contrast-more:border-pearl-ink"
    >
      <InviteIcon aria-hidden="true" size={20} />
      <strong className="mt-1.5 text-[10px]">Your leaderboard is lonely</strong>
      <small className="mt-0.5 max-w-[260px] text-[8px] leading-3.5 text-pearl-muted contrast-more:text-pearl-ink">
        Invite friends by TouchGrass ID to compare scores.
      </small>
      <div className="mt-2">
        <Button onClick={onAddTokenmaxxer} type="button">
          Add a Tokenmaxxer
        </Button>
      </div>
    </div>
  );
}

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

function DoomerboardPodium({ rows }: { rows: readonly DoomerboardRow[] }) {
  const rowsByRank = new Map(rows.map((row) => [row.rank, row]));
  const podium = [
    rowsByRank.get(2),
    rowsByRank.get(1),
    rowsByRank.get(3),
  ].filter((row): row is DoomerboardRow => row !== undefined);
  const ledger = rows
    .filter((row) => row.rank > 3)
    .sort((a, b) => a.rank - b.rank);

  return (
    <ScrollArea
      aria-label="Doomerboard rankings"
      className="h-full"
      data-doomerboard-scroll=""
      viewportClassName="select-none overscroll-contain"
    >
      <div className="grid grid-cols-[1fr_1.12fr_1fr] items-end gap-[5px] px-3.5 pt-[25px] pb-[11px]">
        {podium.map((row) => {
          const style = rankStyles[row.rank as 1 | 2 | 3];
          return (
            <article
              className={`relative flex flex-col items-center rounded-t-[13px] rounded-b-[8px] border px-1 py-2 text-center shadow-rank-card backdrop-blur-[8px] ${style.card}`}
              key={row.id}
            >
              <div
                className={`absolute -top-3.5 grid place-items-center rounded-full font-extrabold shadow-control ${style.medal}`}
              >
                {row.rank}
              </div>
              <span className="mt-[15px] font-mono text-[7px] tracking-[0.08em] text-pearl-muted">
                {row.note}
              </span>
              <b className="mt-auto text-[12px]">{row.name}</b>
              <small className="mt-0.5 text-[7px] text-pearl-muted">
                {row.id}
              </small>
              <strong className="mt-[7px] text-[16px]">{row.score}</strong>
            </article>
          );
        })}
      </div>
      {ledger.length > 0 ? (
        <div
          aria-label="More Doomerboard ranks"
          className="mx-3.5 border-t border-pearl-line"
          data-slot="doomerboard-ledger"
        >
          {ledger.map((row) => (
            <article
              className="grid grid-cols-[30px_1fr_70px] items-center border-b border-pearl-line px-2.5 py-[8px] text-pearl-ink last:border-b-0"
              key={row.id}
            >
              <strong className="text-pearl-muted">{row.rank}</strong>
              <span>
                <b className="block text-[11px]">{row.name}</b>
                <small className="mt-0.5 block text-[7px] text-pearl-muted">
                  {row.id}
                </small>
              </span>
              <b className="text-right text-[12px]">{row.score}</b>
            </article>
          ))}
        </div>
      ) : null}
    </ScrollArea>
  );
}

function Doomerboard({
  currentProfile = null,
  initialAudience = "global",
  onAddTokenmaxxer = () => undefined,
  providers = [],
  rows,
  tokenmaxxerRows,
}: {
  currentProfile?: CurrentUser | null | undefined;
  initialAudience?: Audience | undefined;
  onAddTokenmaxxer?: (() => void) | undefined;
  providers?: readonly DoomerboardProvider[] | undefined;
  rows?: readonly DoomerboardRow[] | undefined;
  tokenmaxxerRows?: readonly DoomerboardRow[] | undefined;
}) {
  const [audience, setAudience] = useState<Audience>(initialAudience);
  const [period, setPeriod] = useState("today");
  const [provider, setProvider] = useState("combined");
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
          ? "Friends rankings"
          : tokenmaxxersEmpty
            ? "Friends empty"
            : globalRowsMatchSelection
              ? "Doomerboard rankings"
              : "Doomerboard unavailable"
      }
      className="pb-2"
    >
      <DoomerboardToolbar
        audience={audience}
        onAudienceChange={setAudience}
        onPeriodChange={setPeriod}
        onProviderChange={setProvider}
        period={period}
        provider={provider}
        providers={providers}
        currentUser={currentProfile}
      />
      <div className="mt-3 h-[180px]" data-slot="doomerboard-viewport">
        {tokenmaxxersMatchSelection ? (
          <DoomerboardPodium rows={tokenmaxxerRows} />
        ) : tokenmaxxersEmpty ? (
          <TokenmaxxersEmpty onAddTokenmaxxer={onAddTokenmaxxer} />
        ) : globalRowsMatchSelection ? (
          <DoomerboardPodium rows={rows} />
        ) : (
          <DoomerboardUnavailable selectionUnavailable={rows !== undefined} />
        )}
      </div>
    </section>
  );
}

export { Doomerboard, TokenmaxxersEmpty };
export type { CurrentUser as CurrentProfile, DoomerboardRow };
