import {
  Brand,
  Button,
  EllipsisIcon,
  MetricCard,
  MetricCardDetail,
  MetricCardGauge,
  MetricCardGroup,
  MetricCardLabel,
  MetricCardTrend,
  MetricCardValue,
  ProviderConnectionCard,
  ProviderMark,
  QuotaProgress,
  SegmentedControl,
  SegmentedControlItem,
} from "@touchgrass/ui";
import { useEffect, useState } from "react";

import grassHandDirect from "@touchgrass/ui/assets/brand/glass-grass-hand.png?url";
import appleLogo from "../assets/providers/apple.svg?url";
import githubLogo from "../assets/providers/github.svg?url";
import xLogo from "../assets/providers/x.svg?url";
import {
  downloadFallbackUrl,
  installDownloadResolver,
} from "../lib/download-resolver";
import { gardenTimeForHour, type GardenTime } from "../lib/garden-time";

type GardenTimeChoice = GardenTime | "auto";

type BoardRow = {
  id: string;
  name: string;
  note: string;
  rank: 1 | 2 | 3 | 4 | 5 | 6;
  score: string;
};

const GARDEN_COPY: Record<GardenTime, [string, string, string]> = {
  dawn: ["The sun is up.", "Apparently,", "so are you."],
  day: ["The garden is", "at 100%.", "You are not."],
  golden: ["Even the sun", "has logged off.", "You have not."],
  night: ["Your agents", "touched grass", "without you."],
};

const GLOBAL_ROWS: BoardRow[] = [
  { id: "#TG-4COLD7", name: "laura", note: "ABSOLUTELY FINE", rank: 1, score: "18.2M" },
  { id: "#TG-7K4P9D", name: "you", note: "YOU", rank: 2, score: "12.8M" },
  { id: "#TG-BURN42", name: "max", note: "STILL ONLINE", rank: 3, score: "9.1M" },
  { id: "#TG-NULL77", name: "nora", note: "", rank: 4, score: "7.8M" },
  { id: "#TG-LAWN01", name: "sam", note: "", rank: 5, score: "6.4M" },
  { id: "#TG-AFK404", name: "eli", note: "", rank: 6, score: "4.9M" },
];

function readGardenTimeChoice(): GardenTimeChoice {
  if (typeof window === "undefined") return "auto";
  const choice = new URLSearchParams(window.location.search).get("time");
  return choice === "dawn" ||
    choice === "day" ||
    choice === "golden" ||
    choice === "night"
    ? choice
    : "auto";
}

function SiteBrand({ reversed = false }: { reversed?: boolean }) {
  return <Brand className={reversed ? "site-brand site-brand--reversed" : "site-brand"} />;
}

function ProviderRow({
  label,
  provider,
  quota,
  value,
}: {
  label: string;
  provider: "claude" | "codex";
  quota: string;
  value: number;
}) {
  return (
    <section className="border-b border-pearl-line bg-provider-row px-4 py-[15px] contrast-more:border-pearl-ink contrast-more:bg-pearl-highlight">
      <header className="grid grid-cols-[auto_1fr_auto] items-center gap-2.5">
        <span className="grid h-[29px] w-[29px] place-items-center overflow-hidden rounded-[7px] border border-input bg-pearl-control shadow-control contrast-more:border-pearl-ink">
          <ProviderMark provider={provider} />
        </span>
        <span className="flex min-w-0 flex-col gap-0.5">
          <strong className="text-[13px]">{label}</strong>
          <small className="truncate text-[10px] text-pearl-muted contrast-more:text-pearl-ink">
            {quota}
          </small>
        </span>
        <strong className="text-[21px]">{value}%</strong>
      </header>
      <div className="mt-3">
        <QuotaProgress
          aria-label={`${label} quota current, ${value} percent remaining`}
          provider={provider}
          value={value}
        />
      </div>
    </section>
  );
}

function UsageOverviewDemo() {
  const metrics = [
    { detail: "≈ $38.61", fill: 38, label: "Today", tone: "today", trend: "+8%", value: "12.8M" },
    { detail: "≈ $214.96", fill: 67, label: "7 days", tone: "week", trend: "+14%", value: "71.4M" },
    { detail: "≈ $856.73", fill: 92, label: "30 days", tone: "month", trend: "+22%", value: "284.6M" },
  ] as const;

  return (
    <section className="border-b border-pearl-line bg-usage-surface px-4 py-[13px] contrast-more:border-pearl-ink contrast-more:bg-pearl-highlight">
      <header className="flex items-center justify-between">
        <h2 className="m-0 text-[10px]">Observed tokens</h2>
        <small className="text-[8px] text-pearl-muted contrast-more:text-pearl-ink">API equivalent</small>
      </header>
      <div className="mt-2">
        <MetricCardGroup>
          {metrics.map((metric) => (
            <MetricCard key={metric.label}>
              <MetricCardLabel>{metric.label}</MetricCardLabel>
              <MetricCardTrend tone="positive">{metric.trend}</MetricCardTrend>
              <MetricCardValue>{metric.value}</MetricCardValue>
              <MetricCardDetail tone="positive">{metric.detail}</MetricCardDetail>
              <MetricCardGauge fill={metric.fill} tone={metric.tone} />
            </MetricCard>
          ))}
        </MetricCardGroup>
      </div>
    </section>
  );
}

const RANK_STYLES = {
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

function BoardRankings({ expanded, rows }: { expanded: boolean; rows: BoardRow[] }) {
  const rowsByRank = new Map(rows.map((row) => [row.rank, row]));
  const podium = [rowsByRank.get(2), rowsByRank.get(1), rowsByRank.get(3)].filter(
    (row): row is BoardRow => row !== undefined,
  );
  const ledger = expanded ? rows.filter((row) => row.rank > 3) : rows.slice(3, 4);

  return (
    <div className="h-full overflow-hidden select-none">
      <div className="grid grid-cols-[1fr_1.12fr_1fr] items-end gap-[5px] px-3.5 pt-[25px] pb-[11px]">
        {podium.map((row) => {
          const style = RANK_STYLES[row.rank as 1 | 2 | 3];
          return (
            <article
              className={`relative flex flex-col items-center rounded-t-[13px] rounded-b-[8px] border px-1 py-2 text-center shadow-rank-card backdrop-blur-[8px] ${style.card}`}
              key={row.id}
            >
              <div className={`absolute -top-3.5 grid place-items-center rounded-full font-extrabold shadow-control ${style.medal}`}>
                {row.rank}
              </div>
              <span className="mt-[15px] font-mono text-[7px] tracking-[0.08em] text-pearl-muted">{row.note}</span>
              <b className="mt-auto text-[12px]">{row.name}</b>
              <small className="mt-0.5 text-[7px] text-pearl-muted">{row.id}</small>
              <strong className="mt-[7px] text-[16px]">{row.score}</strong>
            </article>
          );
        })}
      </div>
      <div className="mx-3.5 border-t border-pearl-line">
        {ledger.map((row) => (
          <article className="grid grid-cols-[30px_1fr_70px] items-center border-b border-pearl-line px-2.5 py-[8px] text-pearl-ink last:border-b-0" key={row.id}>
            <strong className="text-pearl-muted">{row.rank}</strong>
            <span><b className="block text-[11px]">{row.name}</b><small className="mt-0.5 block text-[7px] text-pearl-muted">{row.id}</small></span>
            <b className="text-right text-[12px]">{row.score}</b>
          </article>
        ))}
      </div>
    </div>
  );
}

function DoomerboardSurface({ expanded = false }: { expanded?: boolean }) {
  return (
    <section className="pb-2">
      <header className="flex items-center justify-between px-3.5 pt-3">
        <div className="flex min-w-0 items-center gap-1.5">
          <strong className="shrink-0 text-[10px]">Doomerboard</strong>
          <small className="truncate font-mono text-[7px] text-pearl-muted">you#TG-7K4P9D</small>
        </div>
        <div className="flex items-center gap-0.5 text-pearl-muted">
          <Button size="quiet" type="button" variant="ghost">Today</Button>
          <span aria-hidden="true" className="text-[8px]">·</span>
          <Button size="quiet" type="button" variant="ghost">Combined</Button>
        </div>
      </header>
      <div className="mx-3.5 mt-2">
        <SegmentedControl aria-label="Doomerboard audience" value="global">
          <SegmentedControlItem value="mine">My Tokenmaxxers</SegmentedControlItem>
          <SegmentedControlItem value="global">Global</SegmentedControlItem>
        </SegmentedControl>
      </div>
      <div className={expanded ? "mt-3 h-[330px]" : "mt-3 h-[180px]"}>
        <BoardRankings expanded={expanded} rows={GLOBAL_ROWS} />
      </div>
    </section>
  );
}

function ProductPanel() {
  return (
    <article className="product-panel-preview product-surface overflow-hidden rounded-panel border border-pearl-border bg-panel-glass text-pearl-ink shadow-panel-glass contrast-more:border-pearl-ink contrast-more:bg-pearl-highlight contrast-more:shadow-none" inert>
      <header className="flex items-center justify-between border-b border-pearl-line bg-panel-header px-4 pt-[15px] pb-3 contrast-more:border-pearl-ink">
        <div className="flex min-w-0 items-center gap-2.5">
          <Brand />
          <small className="truncate border-l border-pearl-line pl-2.5 text-[10px] text-pearl-muted contrast-more:border-pearl-ink contrast-more:text-pearl-ink">Synced locally</small>
        </div>
        <Button aria-label="Open panel menu" size="icon" title="Open panel menu" type="button" variant="ghost">
          <EllipsisIcon aria-hidden="true" size={19} />
        </Button>
      </header>
      <ProviderRow label="Codex" provider="codex" quota="5-hour limit · resets 14:40" value={62} />
      <ProviderRow label="Claude" provider="claude" quota="Weekly limit · resets Thu 03:00" value={18} />
      <UsageOverviewDemo />
      <DoomerboardSurface />
    </article>
  );
}

function OnboardingSurface() {
  return (
    <article className="product-onboarding-window product-surface overflow-hidden rounded-[16px] border border-sheet-border bg-native-sheet text-sheet-ink shadow-native-sheet" inert>
      <aside className="border-r border-sheet-line bg-sheet-sidebar px-4 py-7">
        <Brand className="px-2" />
        <nav aria-label="Onboarding steps" className="mt-8 grid gap-1 text-[12px]">
          <span className="grid grid-cols-[22px_1fr] items-center gap-2 rounded-[8px] bg-sheet-active px-2 py-2.5 text-left text-[11px] font-semibold text-sheet-ink"><i>1</i>Providers</span>
          <span className="grid grid-cols-[22px_1fr] items-center gap-2 rounded-[8px] px-2 py-2.5 text-left text-[11px] text-sheet-muted"><i>2</i>Profile</span>
          <span className="grid grid-cols-[22px_1fr] items-center gap-2 rounded-[8px] px-2 py-2.5 text-left text-[11px] text-sheet-muted"><i>3</i>Finish</span>
        </nav>
      </aside>
      <section className="flex min-w-0 flex-col px-12 py-8">
        <small className="font-mono text-[8px] font-semibold tracking-[0.08em] text-sheet-muted uppercase">Step 1 of 3</small>
        <h3 className="mt-2 mb-0 text-[30px] tracking-[-0.045em]">Confirm your Coding Providers</h3>
        <p className="mt-2 mb-4 text-[11px] leading-5 text-sheet-muted">Confirm the Coding Providers that TouchGrassBar detects on this Mac.</p>
        <div className="grid gap-3">
          <ProviderConnectionCard description="Codex was detected on this Mac. No credentials or private provider data were read." label="Codex" provider="codex" status="Detected" />
          <ProviderConnectionCard description="Claude was detected on this Mac. No credentials or private provider data were read." label="Claude" provider="claude" status="Detected" />
        </div>
        <div className="mt-auto flex justify-end border-t border-sheet-line pt-4">
          <Button type="button">Continue</Button>
        </div>
      </section>
    </article>
  );
}

function NightGarden() {
  const [gardenTime, setGardenTime] = useState<GardenTime>("day");
  const [headerScrolled, setHeaderScrolled] = useState(false);
  const [copyStatus, setCopyStatus] = useState<"copied" | "idle" | "unavailable">("idle");
  const heroCopy = GARDEN_COPY[gardenTime];

  useEffect(() => {
    const updateGarden = () => {
      const choice = readGardenTimeChoice();
      setGardenTime(choice === "auto" ? gardenTimeForHour(new Date().getHours()) : choice);
    };
    updateGarden();
    const timer = window.setInterval(updateGarden, 60_000);
    return () => window.clearInterval(timer);
  }, []);

  useEffect(() => {
    const updateHeader = () => setHeaderScrolled(window.scrollY > 24);
    updateHeader();
    window.addEventListener("scroll", updateHeader, { passive: true });
    return () => window.removeEventListener("scroll", updateHeader);
  }, []);

  useEffect(() => {
    void installDownloadResolver(document, window.fetch.bind(window));
  }, []);

  const copyInvite = async () => {
    try {
      await navigator.clipboard.writeText(`${window.location.origin}/join`);
      setCopyStatus("copied");
    } catch {
      setCopyStatus("unavailable");
    }
  };

  const copyLabel = copyStatus === "copied" ? "Copied" : copyStatus === "unavailable" ? "Unavailable" : "Copy";

  return (
    <main className="direction direction-d identity-native" id="main-content">
      <header className={`d-menubar ${headerScrolled ? "scrolled" : ""}`}>
        <div className="d-brand"><SiteBrand reversed /></div>
        <a className="d-header-download" data-download-link href={downloadFallbackUrl}><img alt="" src={appleLogo} />Download for macOS</a>
      </header>

      <section className={`d-garden-hero garden-${gardenTime}`}>
        {(["dawn", "day", "golden", "night"] as const).map((time) => (
          <div className={`d-time-layer ${time === gardenTime ? "active" : ""} time-${time}`} key={time} />
        ))}
        <div className="d-mist-layer" />
        <div className="d-life-layer" aria-hidden="true">{Array.from({ length: 14 }, (_, index) => <i key={index} />)}</div>
        <div className="d-hero-copy">
          <h1>{heroCopy[0]}<br />{heroCopy[1]}<br /><em>{heroCopy[2]}</em></h1>
          <p>A menu bar utility for Codex and Claude. See your limits and compare Observed Tokens on the Doomerboard.</p>
        </div>
        <ProductPanel />
      </section>

      <section className="d-board-stage d-bar-only-stage" id="doomerboard">
        <header className="d-board-stage-copy"><h2>One board to rank them all.<br /><em>The Doomerboard.</em></h2><p>Compare Token Scores in My Tokenmaxxers and Global. Change the Coding Provider or time range in the app.</p></header>
        <div className="product-board-wrap product-surface overflow-hidden rounded-panel border border-pearl-border bg-panel-glass text-pearl-ink shadow-panel-glass" inert>
          <header className="border-b border-pearl-line bg-panel-header px-4 pt-[15px] pb-3"><Brand /></header>
          <DoomerboardSurface expanded />
        </div>
      </section>

      <section className="d-principles-stage" aria-label="Privacy boundary">
        <div><strong>Local provider access.</strong><span>Credentials, sessions, provider content, raw logs, prompts, conversations, and local paths stay on your Mac.</span></div>
        <div><strong>Public score.</strong><span>Only approved Daily Usage Aggregates and public Profile data can take part in a Doomerboard.</span></div>
        <div><strong>No browser account.</strong><span>Bootstrap creates or restores your Profile in the app. TouchGrassBar then stays in the menu bar.</span></div>
      </section>

      <section className="d-recruit-stage specimen">
        <div className="d-recruit-copy"><h2>Bring your<br />Tokenmaxxers.</h2><p>Invite other Tokenmaxxers and find out who can turn the most tokens into the least sunlight. Prompts stay private.</p><div><Button onClick={copyInvite} type="button">Copy invite link</Button><small aria-live="polite">{copyStatus === "unavailable" ? "Copy is unavailable." : "No account required to start locally."}</small></div></div>
        <article className="product-invite-card product-surface"><img alt="" className="product-direct-icon" src={grassHandDirect} /><h3>You’ve been added to a Doomerboard.</h3><p>You’ve got a spot on the Doomerboard. Claim it on your Mac.</p><div><b>touchgrass.bar/join</b><Button onClick={copyInvite} type="button">{copyLabel}</Button></div></article>
      </section>

      <section className="d-bootstrap-stage specimen" id="setup">
        <header><h2>Two providers.<br /><em>One menu bar.</em></h2><p>TouchGrassBar detects Codex and Claude in Applications or your command-line tools on your Mac.</p></header>
        <OnboardingSurface />
      </section>

      <section className="d-release specimen" id="download">
        <div className="d-release-copy"><h2>A tiny monument<br />to your token problem.</h2><p>See how your Codex and Claude Token Scores rank with other Tokenmaxxers.</p><a className="d-macos-download compact" data-download-link href={downloadFallbackUrl}><img alt="" src={appleLogo} />Download for macOS</a></div>
        <article aria-label="TouchGrassBar update preview" className="product-update-sheet product-surface"><img alt="" className="product-direct-icon" src={grassHandDirect} /><h3>An update is available.</h3><p>TouchGrassBar verifies signed updates. The menu bar app restarts after it installs an update.</p><div inert><Button type="button" variant="secondary">Later</Button><Button type="button">Install &amp; Relaunch</Button></div></article>
      </section>

      <footer className="d-footer">
        <div className="d-footer-brand"><SiteBrand reversed /><span>Open Source. Public score. Private work.</span></div>
        <nav aria-label="Project links"><a href="https://github.com/FabienGreard/TouchGrassBar" rel="noreferrer" target="_blank"><img alt="" src={githubLogo} /><span>Star on GitHub</span></a><a href="https://x.com/FabienGreard" rel="noreferrer" target="_blank"><img alt="" src={xLogo} /><span>@FabienGreard</span></a></nav>
      </footer>
    </main>
  );
}

export default function LandingExperience() {
  return <NightGarden />;
}
