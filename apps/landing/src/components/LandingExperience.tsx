import {
  Brand,
  BrandMark,
  Button,
  DesktopAppIcon,
  DoomerboardRankings,
  DoomerboardToolbar,
  EllipsisIcon,
  PanelShell,
  ProviderCard,
  ProviderMark,
  UsageOverview,
  type DoomerboardRow,
} from "@touchgrass/ui";
import type {
  ProviderPresentation,
  UsagePeriods,
} from "@touchgrass/contracts";
import {
  type CSSProperties,
  type ReactNode,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from "react";

import appleLogo from "../assets/providers/apple.svg?url";
import githubLogo from "../assets/providers/github.svg?url";
import xLogo from "../assets/providers/x.svg?url";
import {
  downloadFallbackUrl,
  installDownloadResolver,
} from "../lib/download-resolver";
import {
  GARDEN_COPY,
  gardenTimeForHour,
  isGardenTime,
  type GardenTime,
} from "../lib/garden-time";

type GardenTimeChoice = GardenTime | "auto";
type LandingExperienceProps = {
  initialGardenTime?: GardenTime | undefined;
  invitation?: boolean;
};

type ProductSpecimenProps = {
  children: ReactNode;
  className: string;
  designWidth: number;
};

type ProductSpecimenStyle = CSSProperties & {
  "--product-specimen-width": string;
};

const GLOBAL_ROWS: DoomerboardRow[] = [
  { displayName: "laura", note: "ABSOLUTELY FINE", rank: 1, tokenScore: "18.2M", touchGrassId: "#TG-4COLD7" },
  { displayName: "Fabien", note: "PROMPT ENJOYER", rank: 2, tokenScore: "12.8M", touchGrassId: "#TG-7K4P9D" },
  { displayName: "max", note: "STILL ONLINE", rank: 3, tokenScore: "9.1M", touchGrassId: "#TG-BURN42" },
  { displayName: "nora", rank: 4, tokenScore: "7.8M", touchGrassId: "#TG-NULL77" },
  { displayName: "eli", rank: 5, tokenScore: "6.4M", touchGrassId: "#TG-LOOP55" },
  { displayName: "mia", rank: 6, tokenScore: "4.9M", touchGrassId: "#TG-DIM420" },
  { displayName: "theo", rank: 7, tokenScore: "3.8M", touchGrassId: "#TG-GRASS7" },
  { displayName: "zara", rank: 8, tokenScore: "2.4M", touchGrassId: "#TG-SLEEP8" },
];

const SCOREBOARD_ROWS: DoomerboardRow[] = [
  { displayName: "laura", note: "ABSOLUTELY FINE", rank: 1, tokenScore: "18.2M", touchGrassId: "#TG-4COLD7" },
  { displayName: "nora", note: "PROMPT ENJOYER", rank: 2, tokenScore: "12.8M", touchGrassId: "#TG-NULL77" },
  { displayName: "max", note: "STILL ONLINE", rank: 3, tokenScore: "9.1M", touchGrassId: "#TG-BURN42" },
];

const observedAt = "2026-08-07T12:00:00.000Z";
const panelReferenceTime = "2026-08-08T12:00:00.000Z";
const panelTimeZone = "Europe/Paris";

function observedUsage(
  observedTokens: number,
  apiEquivalentCostUsd: number,
  trendPercent: number,
) {
  return {
    apiEquivalentCostBasis: "published-provider-pricing",
    apiEquivalentCostCoveragePercent: null,
    apiEquivalentCostQuality: "reconciled" as const,
    apiEquivalentCostUsd,
    availability: "current" as const,
    coverage: "complete" as const,
    evidenceBasis: "provider-reported" as const,
    observedAt,
    observedTokens,
    trendPercent,
  };
}

const PANEL_USAGE: UsagePeriods = {
  scanStatus: "complete",
  sevenDays: observedUsage(71_400_000, 214.96, 14),
  thirtyDays: observedUsage(284_600_000, 856.73, 22),
  today: observedUsage(12_800_000, 38.61, -8),
};

const unavailableUsage: UsagePeriods = {
  scanStatus: "unavailable",
  sevenDays: { availability: "unavailable" },
  thirtyDays: { availability: "unavailable" },
  today: { availability: "unavailable" },
};

const PANEL_PROVIDERS: ProviderPresentation[] = [
  {
    displayName: "Codex",
    presence: "detected",
    provider: "codex",
    quota: {
      availability: "current",
      observedAt,
      provider: "codex",
      quotaLanes: [
        { allowance: 100, label: "Weekly limit", remaining: 74, resetAt: "2026-08-12T14:00:00.000Z", unit: "percent" },
        { allowance: 100, label: "5-hour limit", remaining: 62, resetAt: "2026-08-08T14:40:00.000Z", unit: "percent" },
      ],
    },
    usage: PANEL_USAGE,
  },
  {
    displayName: "Claude",
    presence: "detected",
    provider: "claude",
    quota: {
      availability: "current",
      observedAt,
      provider: "claude",
      quotaLanes: [
        { allowance: 100, label: "Weekly limit", remaining: 18, resetAt: "2026-08-13T03:00:00.000Z", unit: "percent" },
        { allowance: 100, label: "5-hour limit", remaining: 43, resetAt: "2026-08-08T14:20:00.000Z", unit: "percent" },
      ],
    },
    usage: unavailableUsage,
  },
];

const PANEL_USAGE_PRESENTATION = {
  sevenDays: { gaugeFill: 64, trend: "+14%", trendDescription: "Up 14 percent from the previous 7 days" },
  thirtyDays: { gaugeFill: 100, trend: "+22%", trendDescription: "Up 22 percent from the previous 30 days" },
  today: { gaugeFill: 34, trend: "-8%", trendDescription: "Down 8 percent from the previous day" },
};

const noOp = () => undefined;

function ProductSpecimen({
  children,
  className,
  designWidth,
}: ProductSpecimenProps) {
  const canvasRef = useRef<HTMLDivElement>(null);
  const frameRef = useRef<HTMLDivElement>(null);

  useLayoutEffect(() => {
    const canvas = canvasRef.current;
    const frame = frameRef.current;
    if (!canvas || !frame) return;

    let animationFrame = 0;
    let previousCanvasHeight = -1;
    let previousFrameWidth = -1;
    const updateScale = () => {
      window.cancelAnimationFrame(animationFrame);
      animationFrame = window.requestAnimationFrame(() => {
        const canvasHeight = canvas.offsetHeight;
        const frameWidth = frame.clientWidth;
        if (
          canvasHeight === previousCanvasHeight &&
          frameWidth === previousFrameWidth
        ) {
          return;
        }

        previousCanvasHeight = canvasHeight;
        previousFrameWidth = frameWidth;
        const scale = Math.min(1, frameWidth / designWidth);
        frame.style.setProperty("--product-specimen-scale", String(scale));
        frame.style.height = `${canvasHeight * scale}px`;
      });
    };

    const resizeObserver = new ResizeObserver(updateScale);
    resizeObserver.observe(canvas);
    resizeObserver.observe(frame);
    updateScale();

    return () => {
      window.cancelAnimationFrame(animationFrame);
      resizeObserver.disconnect();
    };
  }, [designWidth]);

  return (
    <div
      className={`product-specimen-frame ${className}`}
      ref={frameRef}
      style={
        {
          "--product-specimen-width": `${designWidth}px`,
        } as ProductSpecimenStyle
      }
    >
      <div className="product-specimen-canvas" ref={canvasRef}>
        {children}
      </div>
    </div>
  );
}

function readGardenTimeChoice(): GardenTimeChoice {
  if (typeof window === "undefined") return "auto";
  const choice = new URLSearchParams(window.location.search).get("time");
  return isGardenTime(choice) ? choice : "auto";
}

function initialGardenTimeForRender(initialGardenTime?: GardenTime): GardenTime {
  if (initialGardenTime) return initialGardenTime;
  if (typeof document !== "undefined") {
    const prepaintTime = document.documentElement.dataset.gardenTime;
    if (isGardenTime(prepaintTime)) return prepaintTime;
  }
  return "day";
}

function SiteBrand({ reversed = false }: { reversed?: boolean }) {
  return <Brand className={reversed ? "site-brand site-brand--reversed" : "site-brand"} />;
}

function DoomerboardSurface() {
  return (
    <section className="pb-2">
      <DoomerboardToolbar
        audience="global"
        currentProfile={{
          displayName: "Fabien",
          touchGrassId: "#TG-7K4P9D",
        }}
        onAudienceChange={noOp}
        onCopyCurrentProfile={noOp}
        onPeriodChange={noOp}
        onProviderChange={noOp}
        period="today"
        provider="combined"
        providers={PANEL_PROVIDERS}
      />
      <div className="mt-3 h-[180px]" data-slot="doomerboard-viewport">
        <DoomerboardRankings rows={GLOBAL_ROWS} />
      </div>
    </section>
  );
}

function DoomerboardScoreboard() {
  return (
    <section className="d-scoreboard-showcase" aria-label="Global Doomerboard highlights">
      <header>
        <strong><span aria-hidden="true" />Global Doomerboard</strong>
        <small>Today · Combined</small>
      </header>
      <ol>
        {SCOREBOARD_ROWS.map((row) => (
          <li data-rank={row.rank} key={row.touchGrassId}>
            <span className="d-scoreboard-rank">{String(row.rank).padStart(2, "0")}</span>
            <span className="d-scoreboard-profile">
              <small>{row.note}</small>
              <strong>{row.displayName}</strong>
              <em>{row.touchGrassId}</em>
            </span>
            <b>{row.tokenScore}</b>
          </li>
        ))}
      </ol>
      <footer>
        <span>Public Token Scores</span>
        <span>Prompts and provider data stay on your Mac</span>
      </footer>
    </section>
  );
}

function ProductPanel({ className = "product-panel-preview" }: { className?: string }) {
  return (
    <ProductSpecimen className={className} designWidth={402}>
      <PanelShell className="product-panel-canvas product-surface" glass inert>
          <header className="flex items-center justify-between border-b border-pearl-line bg-panel-header px-4 pt-[15px] pb-3 contrast-more:border-pearl-ink">
            <div className="flex min-w-0 items-center gap-2.5">
              <Brand />
              <small className="truncate border-l border-pearl-line pl-2.5 text-[10px] text-pearl-muted contrast-more:border-pearl-ink contrast-more:text-pearl-ink">Live</small>
            </div>
            <Button aria-label="Open panel menu" size="icon" title="Open panel menu" type="button" variant="ghost">
              <EllipsisIcon aria-hidden="true" size={19} />
            </Button>
          </header>
          {PANEL_PROVIDERS.map((provider) => (
            <ProviderCard
              key={provider.provider}
              presentation={provider}
              referenceTime={panelReferenceTime}
              timeZone={panelTimeZone}
            />
          ))}
          <UsageOverview
            presentation={PANEL_USAGE_PRESENTATION}
            topModelUsage={{ model: "GPT 5.6 Sol", observedTokens: 12_800_000 }}
            usage={PANEL_USAGE}
          />
          <DoomerboardSurface />
      </PanelShell>
    </ProductSpecimen>
  );
}

function SetupCopy() {
  return <header><h2><span>Two providers.</span><br /><em>One menu bar.</em></h2><p>Codex and Claude, detected locally on your Mac.</p></header>;
}

function SetupSection() {
  return (
    <section className="d-bootstrap-stage d-setup-section specimen" id="setup">
      <SetupCopy />
      <div className="d-setup-ledger" aria-label="Two Coding Providers listed above the TouchGrassBar result">
        <article><ProviderMark provider="codex" size="large" /><b>Codex</b><span>Detected locally</span></article>
        <article><ProviderMark provider="claude" size="large" /><b>Claude</b><span>Detected locally</span></article>
        <article className="result"><BrandMark aria-hidden="true" decoding="async" loading="lazy" tone="ink" /><b>TouchGrassBar</b><span>One quiet place</span></article>
      </div>
    </section>
  );
}

function InviteVariantD() {
  return (
    <div className="d-invite-d">
      <div className="d-invite-note-rain" aria-hidden="true">
        <span>“One more prompt.”</span>
        <span>“No need to review.”</span>
        <span>“The agent understands.”</span>
        <span>“It’s not a loop. It’s orchestration.”</span>
        <span>“It’s a reasoning graph.”</span>
        <span>“Human in the loop. Eventually.”</span>
        <span>“<b className="d-diff-add">+24,982</b> <b className="d-diff-delete">−842</b>. LGTM.”</span>
        <span>“We reinvented while(true).”</span>
        <span>“The diff has a scrollbar.”</span>
        <span>“That’s good AI slop right there.”</span>
        <span>“Is it AGI yet?”</span>
        <span>“We need an evaluator agent.”</span>
        <span>“I approve for a living.”</span>
        <span>“That’s not what I asked.”</span>
        <span>“Glad I learned algorithms.”</span>
        <span>“10x engineer. $200/month.”</span>
        <span>“Who wrote this? Yes.”</span>
        <span>“CI is green again.”</span>
        <span>“The hallucination compiles.”</span>
        <span>“No AI was harmed writing this.”</span>
      </div>
      <header><h2>Vibe code alone.<br /><em>Tokenmaxx together.</em></h2><p>Add your friends. Compare Token Scores. Keep every prompt private.</p><a className="d-macos-download" data-analytics-event="download clicked" data-analytics-placement="invite" data-download-link href={downloadFallbackUrl}><img alt="" src={appleLogo} />Download for macOS</a></header>
    </div>
  );
}

function InviteSection() {
  return (
    <section className="d-recruit-stage d-invite-variant-d specimen">
      <InviteVariantD />
    </section>
  );
}

function TouchGrassInstallItem() {
  return (
    <div className="d-install-item d-install-touchgrass">
      <DesktopAppIcon className="d-install-app-icon" decoding="async" loading="lazy" size="large" />
      <strong>TouchGrassBar</strong>
      <small>Drag to install</small>
    </div>
  );
}

function ApplicationsInstallItem() {
  return (
    <div className="d-install-item d-install-applications">
      <span className="d-applications-folder" aria-hidden="true"><b>A</b></span>
      <strong>Applications</strong>
      <small>Then open from the menu bar</small>
    </div>
  );
}

function DownloadSection() {
  return (
    <section className="d-release specimen" id="download">
      <div className="d-release-copy">
        <span>Ready when your Mac is</span>
        <h2><span>A tiny monument</span><span>to your token problem.</span></h2>
        <p>See how your Codex and Claude Token Scores rank with other Tokenmaxxers.</p>
        <a className="d-macos-download compact" data-analytics-event="download clicked" data-analytics-placement="release" data-download-link href={downloadFallbackUrl}><img alt="" src={appleLogo} />Download for macOS</a>
      </div>
      <div className="d-release-install" aria-label="Install TouchGrassBar in Applications">
        <TouchGrassInstallItem />
        <span className="d-install-arrow" aria-hidden="true">→</span>
        <ApplicationsInstallItem />
      </div>
    </section>
  );
}

function NightGarden({ initialGardenTime, invitation = false }: LandingExperienceProps) {
  const [gardenTime, setGardenTime] = useState<GardenTime>(() =>
    initialGardenTimeForRender(initialGardenTime),
  );
  const [suppressTimeFade, setSuppressTimeFade] = useState(true);
  const [headerScrolled, setHeaderScrolled] = useState(false);
  const heroCopy = invitation
    ? (["Install the app.", "Create your Profile.", "Join the board."] as const)
    : GARDEN_COPY[gardenTime];

  useEffect(() => {
    const updateGarden = () => {
      const choice = readGardenTimeChoice();
      const nextGardenTime = choice === "auto" ? gardenTimeForHour(new Date().getHours()) : choice;
      document.documentElement.dataset.gardenTime = nextGardenTime;
      setGardenTime(nextGardenTime);
    };
    updateGarden();
    const timer = window.setInterval(updateGarden, 60_000);
    return () => window.clearInterval(timer);
  }, []);

  useEffect(() => {
    if (!suppressTimeFade) return;
    const frame = window.requestAnimationFrame(() => setSuppressTimeFade(false));
    return () => window.cancelAnimationFrame(frame);
  }, [suppressTimeFade]);

  useEffect(() => {
    const updateHeader = () => setHeaderScrolled(window.scrollY > 24);
    updateHeader();
    window.addEventListener("scroll", updateHeader, { passive: true });
    return () => window.removeEventListener("scroll", updateHeader);
  }, []);

  useEffect(() => {
    void installDownloadResolver(document, window.fetch.bind(window));
  }, []);

  return (
    <main className="direction direction-d identity-native" id="main-content">
      <header className={`d-menubar ${headerScrolled ? "scrolled" : ""}`}>
        <div className="d-brand"><SiteBrand reversed /></div>
        <a className="d-header-download" data-analytics-event="download clicked" data-analytics-placement="header" data-download-link href={downloadFallbackUrl}><img alt="" src={appleLogo} />Download for macOS</a>
      </header>

      <section className={`d-garden-hero garden-${gardenTime} ${suppressTimeFade ? "d-time-instant" : ""}`} suppressHydrationWarning>
        {(["dawn", "day", "golden", "night"] as const).map((time) => (
          <div
            className={`d-time-layer ${time === gardenTime ? "active" : ""} time-${time}`}
            data-garden-layer={time}
            key={time}
            suppressHydrationWarning
          />
        ))}
        <div className="d-mist-layer" />
        <div className="d-life-layer" aria-hidden="true">{Array.from({ length: 24 }, (_, index) => <i className={index >= 14 ? "day-only" : undefined} key={index} />)}</div>
        <div className="d-hero-inner">
          <div className="d-hero-copy">
            <span>{invitation ? "Doomerboard invitation" : "Built for Codex & Claude"}</span>
            <h1><span data-garden-copy-line="0" suppressHydrationWarning>{heroCopy[0]}</span><br /><span data-garden-copy-line="1" suppressHydrationWarning>{heroCopy[1]}</span><br /><em data-garden-copy-line="2" suppressHydrationWarning>{heroCopy[2]}</em></h1>
            <p>{invitation ? "Install TouchGrassBar on your Mac, create or restore your Profile, then join the Doomerboard with your TouchGrass ID." : "Lives in your menu bar. See your limits and compare Observed Tokens on the Doomerboard."}</p>
            <a className="d-macos-download" data-analytics-event="download clicked" data-analytics-placement="hero" data-download-link href={downloadFallbackUrl}><img alt="" src={appleLogo} />Download for macOS</a>
          </div>
          <ProductPanel />
        </div>
      </section>

      <section className="d-board-stage d-bar-only-stage" id="doomerboard">
        <header className="d-board-stage-copy"><h2>One board to rank them all.<br /><em>The Doomerboard.</em></h2><p>See who burned the most tokens and who still remembers daylight.</p></header>
        <DoomerboardScoreboard />
      </section>

      <InviteSection />

      <SetupSection />

      <DownloadSection />

      <footer className="d-footer">
        <div className="d-footer-brand"><SiteBrand reversed /><span>Open Source. Public score. Private work.</span></div>
        <nav aria-label="Project links"><a data-analytics-event="outbound link clicked" data-analytics-placement="github" href="https://github.com/FabienGreard/TouchGrassBar" rel="noreferrer" target="_blank"><img alt="" src={githubLogo} /><span>Star on GitHub</span></a><a data-analytics-event="outbound link clicked" data-analytics-placement="x" href="https://x.com/FabienGreard" rel="noreferrer" target="_blank"><img alt="" src={xLogo} /><span>@FabienGreard</span></a></nav>
      </footer>
    </main>
  );
}

export default function LandingExperience({ initialGardenTime, invitation = false }: LandingExperienceProps) {
  return <NightGarden initialGardenTime={initialGardenTime} invitation={invitation} />;
}
