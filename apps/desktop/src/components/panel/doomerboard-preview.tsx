import {
  Button,
  InviteIcon,
  PanelMenu,
  PanelMenuContent,
  PanelMenuRadioGroup,
  PanelMenuRadioItem,
  PanelMenuTrigger,
  RankingIcon,
  SegmentedControl,
  SegmentedControlItem,
} from "@touchgrass/ui";
import { useEffect, useState } from "react";

import type { DoomerboardPreviewRow } from "../../previewFixtures";

const periodOptions = [
  { label: "Today", value: "today" },
  { label: "7 days", value: "week" },
  { label: "30 days", value: "month" },
] as const;

const providerOptions = [
  { label: "Combined", value: "combined" },
  { label: "Codex", value: "codex" },
  { label: "Claude", value: "claude" },
] as const;

type QueryOption = { label: string; value: string };

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
          variant="quiet"
        >
          {selectedLabel}
        </Button>
      </PanelMenuTrigger>
      <PanelMenuContent align="end" side="top" sideOffset={9} size="query">
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

type CurrentUser = Pick<DoomerboardPreviewRow, "id" | "name">;

type DoomerboardToolbarProps = {
  audience: Audience;
  onAudienceChange: (audience: Audience) => void;
  onPeriodChange: (period: string) => void;
  onProviderChange: (provider: string) => void;
  period: string;
  provider: string;
  currentUser: CurrentUser | null;
};

function CurrentUserIdentity({
  currentUser,
}: {
  currentUser: CurrentUser | null;
}) {
  const [copyStatus, setCopyStatus] = useState<
    "idle" | "copied" | "unavailable"
  >("idle");

  useEffect(() => {
    if (copyStatus === "idle") {
      return undefined;
    }

    const resetTimer = window.setTimeout(() => setCopyStatus("idle"), 1600);
    return () => window.clearTimeout(resetTimer);
  }, [copyStatus]);

  if (currentUser === null) {
    return (
      <small
        aria-label="Current user identity unavailable"
        className="font-mono text-[7px] text-cream-muted"
        data-slot="current-user-identity"
      >
        Identity unavailable
      </small>
    );
  }

  const identity = `${currentUser.name}${currentUser.id}`;
  const copyIdentity = async () => {
    try {
      await navigator.clipboard.writeText(identity);
      setCopyStatus("copied");
    } catch {
      setCopyStatus("unavailable");
    }
  };

  return (
    <span
      className="inline-flex min-w-0 items-center gap-0.5"
      data-slot="current-user-identity"
    >
      <Button
        aria-label={`Copy current user identity ${identity}`}
        data-copy-status={copyStatus}
        data-slot="current-user-identity-action"
        onClick={() => void copyIdentity()}
        size="identity"
        title={
          copyStatus === "copied"
            ? "Copied"
            : copyStatus === "unavailable"
              ? "Copy unavailable"
              : "Copy identity"
        }
        type="button"
        variant="quiet"
      >
        <span className="truncate">{identity}</span>
      </Button>
      <span
        aria-live="polite"
        className="font-mono text-[7px] text-cream-ink"
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
  currentUser,
}: DoomerboardToolbarProps) {
  return (
    <>
      <header className="flex items-center justify-between px-3.5 pt-3">
        <div className="flex min-w-0 items-center gap-1.5">
          <strong className="shrink-0 text-[10px]">Doomerboard</strong>
          <CurrentUserIdentity currentUser={currentUser} />
        </div>
        <div className="flex items-center gap-0.5 text-cream-muted">
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
          <SegmentedControlItem value="mine">Tokenmaxxers</SegmentedControlItem>
          <SegmentedControlItem value="global">Global</SegmentedControlItem>
        </SegmentedControl>
      </div>
    </>
  );
}

function DoomerboardUnavailable({
  onAddFriend,
  previewSelection = false,
}: {
  onAddFriend: () => void;
  previewSelection?: boolean;
}) {
  return (
    <div
      aria-label="Doomerboard unavailable"
      className="mx-3.5 flex h-full flex-col items-center justify-center rounded-[12px] border border-dashed border-cream-line bg-cream-surface px-6 py-3.5 text-center shadow-surface contrast-more:border-cream-ink"
    >
      <RankingIcon aria-hidden="true" size={20} />
      <strong className="mt-1.5 text-[10px]">Doomerboard unavailable</strong>
      <small className="mt-0.5 max-w-[260px] text-[8px] leading-3.5 text-cream-muted contrast-more:text-cream-ink">
        {previewSelection
          ? "This development fixture only covers Global · 30 days · Combined."
          : "Identity and synchronized scores are not ready."}
      </small>
      <div className="mt-2">
        <Button onClick={onAddFriend} type="button">
          Add by ID
        </Button>
      </div>
    </div>
  );
}

function TokenmaxxersEmpty({
  onAddFriend = () => undefined,
}: {
  onAddFriend?: (() => void) | undefined;
}) {
  return (
    <div
      aria-label="Tokenmaxxers empty"
      className="mx-3.5 flex h-full flex-col items-center justify-center rounded-[12px] border border-dashed border-cream-line bg-cream-surface px-6 py-3.5 text-center shadow-surface contrast-more:border-cream-ink"
    >
      <InviteIcon aria-hidden="true" size={20} />
      <strong className="mt-1.5 text-[10px]">Your board is waiting</strong>
      <small className="mt-0.5 max-w-[260px] text-[8px] leading-3.5 text-cream-muted contrast-more:text-cream-ink">
        Invite your friends to join your tokenmaxxers
      </small>
      <div className="mt-2">
        <Button onClick={onAddFriend} type="button">
          Invite a friend
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

function DoomerboardPodium({
  rows,
}: {
  rows: readonly DoomerboardPreviewRow[];
}) {
  const rowsByRank = new Map(rows.map((row) => [row.rank, row]));
  const podium = [
    rowsByRank.get(2),
    rowsByRank.get(1),
    rowsByRank.get(3),
  ].filter((row): row is DoomerboardPreviewRow => row !== undefined);
  const ledger = rows
    .filter((row) => row.rank > 3)
    .sort((a, b) => a.rank - b.rank);

  return (
    <div
      aria-label="Doomerboard rankings"
      className="h-full select-none overflow-y-auto overscroll-contain [scrollbar-color:var(--cream-muted)_transparent] [scrollbar-width:thin] [&::-webkit-scrollbar]:w-1 [&::-webkit-scrollbar-thumb]:rounded-full [&::-webkit-scrollbar-thumb]:bg-cream-ink/15"
      data-preview-fixture="doomerboard"
      data-slot="doomerboard-scroll"
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
              <span className="mt-[15px] font-mono text-[7px] tracking-[0.08em] text-cream-muted">
                {row.note}
              </span>
              <b className="mt-auto text-[12px]">{row.name}</b>
              <small className="mt-0.5 text-[7px] text-cream-muted">
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
          className="mx-3.5 border-t border-cream-line"
          data-slot="doomerboard-ledger"
        >
          {ledger.map((row) => (
            <article
              className="grid grid-cols-[30px_1fr_70px] items-center border-b border-cream-line px-2.5 py-[8px] text-cream-ink last:border-b-0"
              key={row.id}
            >
              <strong className="text-cream-muted">{row.rank}</strong>
              <span>
                <b className="block text-[11px]">{row.name}</b>
                <small className="mt-0.5 block text-[7px] text-cream-muted">
                  {row.id}
                </small>
              </span>
              <b className="text-right text-[12px]">{row.score}</b>
            </article>
          ))}
        </div>
      ) : null}
    </div>
  );
}

function DoomerboardPreview({
  initialAudience = "global",
  onAddFriend = () => undefined,
  previewRows,
  tokenmaxxerPreviewRows,
}: {
  initialAudience?: Audience | undefined;
  onAddFriend?: (() => void) | undefined;
  previewRows?: readonly DoomerboardPreviewRow[] | undefined;
  tokenmaxxerPreviewRows?: readonly DoomerboardPreviewRow[] | undefined;
}) {
  const [audience, setAudience] = useState<Audience>(initialAudience);
  const [period, setPeriod] = useState("today");
  const [provider, setProvider] = useState("combined");
  const fixtureMatchesSelection =
    previewRows !== undefined &&
    audience === "global" &&
    period === "today" &&
    provider === "combined";
  const tokenmaxxersMatchSelection =
    tokenmaxxerPreviewRows !== undefined &&
    audience === "mine" &&
    period === "today" &&
    provider === "combined";
  const tokenmaxxersEmpty =
    audience === "mine" && tokenmaxxerPreviewRows === undefined;
  const currentUser =
    previewRows?.find(
      (row) =>
        row.note?.toUpperCase() === "YOU" || row.name.toLowerCase() === "you",
    ) ?? null;

  return (
    <section
      aria-label={
        tokenmaxxersMatchSelection
          ? "Tokenmaxxers preview fixture"
          : tokenmaxxersEmpty
          ? "Tokenmaxxers empty"
          : fixtureMatchesSelection
            ? "Doomerboard preview fixture"
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
        currentUser={currentUser}
      />
      <div className="mt-3 h-[180px]" data-slot="doomerboard-viewport">
        {tokenmaxxersMatchSelection ? (
          <DoomerboardPodium rows={tokenmaxxerPreviewRows} />
        ) : tokenmaxxersEmpty ? (
          <TokenmaxxersEmpty onAddFriend={onAddFriend} />
        ) : fixtureMatchesSelection ? (
          <DoomerboardPodium rows={previewRows} />
        ) : (
          <DoomerboardUnavailable
            onAddFriend={onAddFriend}
            previewSelection={previewRows !== undefined}
          />
        )}
      </div>
    </section>
  );
}

export { DoomerboardPreview, TokenmaxxersEmpty };
