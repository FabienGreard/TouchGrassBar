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
import type { ProviderPresentation, UsagePeriods } from "@touchgrass/contracts";
import { type CSSProperties, type ReactNode } from "react";

import appleLogo from "../assets/providers/apple.svg?url";
import githubLogo from "../assets/providers/github.svg?url";
import xLogo from "../assets/providers/x.svg?url";
import { downloadFallbackUrl } from "../lib/download-resolver";
import { GARDEN_COPY, isGardenTime, type GardenTime } from "../lib/garden-time";

type LandingExperienceProps = {
  initialGardenTime?: GardenTime | undefined;
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
  {
    apiEquivalentCost: "≈ $214.96",
    displayName: "laura",
    note: "ABSOLUTELY FINE",
    rank: 1,
    tokenScore: "18.2M",
    touchGrassId: "#TG-4COLD7",
  },
  {
    apiEquivalentCost: "≈ $151.84",
    displayName: "nora",
    note: "PROMPT ENJOYER",
    rank: 2,
    tokenScore: "12.8M",
    touchGrassId: "#TG-NULL77",
  },
  {
    apiEquivalentCost: "≈ $108.03",
    displayName: "max",
    note: "STILL ONLINE",
    rank: 3,
    tokenScore: "9.1M",
    touchGrassId: "#TG-BURN42",
  },
  {
    apiEquivalentCost: "≈ $92.41",
    displayName: "jules",
    rank: 4,
    tokenScore: "7.8M",
    touchGrassId: "#TG-BRANCH4",
  },
  {
    apiEquivalentCost: "≈ $75.86",
    displayName: "eli",
    rank: 5,
    tokenScore: "6.4M",
    touchGrassId: "#TG-LOOP55",
  },
  {
    apiEquivalentCost: "≈ $58.07",
    displayName: "mia",
    rank: 6,
    tokenScore: "4.9M",
    touchGrassId: "#TG-DIM420",
  },
  {
    apiEquivalentCost: "≈ $45.04",
    displayName: "theo",
    rank: 7,
    tokenScore: "3.8M",
    touchGrassId: "#TG-GRASS7",
  },
  {
    apiEquivalentCost: "≈ $28.44",
    displayName: "zara",
    rank: 8,
    tokenScore: "2.4M",
    touchGrassId: "#TG-SLEEP8",
  },
];

const SCOREBOARD_ROWS: DoomerboardRow[] = [
  {
    apiEquivalentCost: "≈ $214.96",
    displayName: "laura",
    note: "ABSOLUTELY FINE",
    rank: 1,
    tokenScore: "18.2M",
    touchGrassId: "#TG-4COLD7",
  },
  {
    apiEquivalentCost: "≈ $151.84",
    displayName: "nora",
    note: "PROMPT ENJOYER",
    rank: 2,
    tokenScore: "12.8M",
    touchGrassId: "#TG-NULL77",
  },
  {
    apiEquivalentCost: "≈ $108.03",
    displayName: "max",
    note: "STILL ONLINE",
    rank: 3,
    tokenScore: "9.1M",
    touchGrassId: "#TG-BURN42",
  },
];

const observedAt = "2026-08-07T12:00:00.000Z";
const panelReferenceTime = "2026-08-08T12:00:00.000Z";
const panelTimeZone = "Europe/Paris";

function observedUsage(observedTokens: number, apiEquivalentCostUsd: number, trendPercent: number) {
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
        {
          allowance: 100,
          label: "Weekly limit",
          remaining: 74,
          resetAt: "2026-08-12T14:00:00.000Z",
          unit: "percent",
        },
        {
          allowance: 100,
          label: "5-hour limit",
          remaining: 62,
          resetAt: "2026-08-08T14:40:00.000Z",
          unit: "percent",
        },
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
        {
          allowance: 100,
          label: "Weekly limit",
          remaining: 18,
          resetAt: "2026-08-13T03:00:00.000Z",
          unit: "percent",
        },
        {
          allowance: 100,
          label: "5-hour limit",
          remaining: 43,
          resetAt: "2026-08-08T14:20:00.000Z",
          unit: "percent",
        },
      ],
    },
    usage: unavailableUsage,
  },
];

const PANEL_USAGE_PRESENTATION = {
  sevenDays: {
    gaugeFill: 64,
    trend: "+14%",
    trendDescription: "Up 14 percent from the previous 7 days",
  },
  thirtyDays: {
    gaugeFill: 100,
    trend: "+22%",
    trendDescription: "Up 22 percent from the previous 30 days",
  },
  today: { gaugeFill: 34, trend: "-8%", trendDescription: "Down 8 percent from the previous day" },
};

const noOp = () => undefined;

function ProductSpecimen({ children, className, designWidth }: ProductSpecimenProps) {
  return (
    <div
      className={`product-specimen-frame ${className}`}
      style={
        {
          "--product-specimen-width": `${designWidth}px`,
        } as ProductSpecimenStyle
      }
    >
      <div className="product-specimen-canvas">{children}</div>
    </div>
  );
}

function initialGardenTimeForRender(initialGardenTime?: GardenTime): GardenTime {
  if (initialGardenTime) return initialGardenTime;
  if (typeof document !== "undefined") {
    const prepaintTime = document.documentElement.dataset.gardenTime;
    if (isGardenTime(prepaintTime)) return prepaintTime;
  }
  return "day";
}

function SiteBrand({
  loading,
  reversed = false,
}: {
  loading?: "eager" | "lazy";
  reversed?: boolean;
}) {
  return (
    <Brand
      className={reversed ? "site-brand site-brand--reversed" : "site-brand"}
      markProps={{ loading }}
    />
  );
}

function DoomerboardSurface() {
  return (
    <section className="pb-2">
      <DoomerboardToolbar
        audience="global"
        currentProfile={{
          displayName: "nora",
          touchGrassId: "#TG-NULL77",
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
    <section className="d-scoreboard-showcase" aria-label="Doomerboard highlights">
      <header>
        <strong>
          <span aria-hidden="true" />
          Doomerboard
        </strong>
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
            <b>
              {row.tokenScore}
              {row.apiEquivalentCost ? <small>{row.apiEquivalentCost}</small> : null}
            </b>
          </li>
        ))}
      </ol>
      <footer>
        <span>Public Token Usage</span>
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
            <small className="truncate border-l border-pearl-line pl-2.5 text-[10px] text-pearl-muted contrast-more:border-pearl-ink contrast-more:text-pearl-ink">
              Live
            </small>
          </div>
          <Button
            aria-label="Open panel menu"
            size="icon"
            title="Open panel menu"
            type="button"
            variant="ghost"
          >
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
  return (
    <header>
      <h2>
        <span>Two providers.</span>
        <br />
        <em>One menu bar.</em>
      </h2>
      <p>Codex and Claude, detected locally on your Mac.</p>
    </header>
  );
}

function SetupSection() {
  return (
    <section className="d-bootstrap-stage d-setup-section specimen" id="setup">
      <SetupCopy />
      <div
        className="d-setup-ledger"
        aria-label="Two Coding Providers listed above the TouchGrassBar result"
      >
        <article>
          <ProviderMark provider="codex" size="large" />
          <b>Codex</b>
          <span>Detected locally</span>
        </article>
        <article>
          <ProviderMark provider="claude" size="large" />
          <b>Claude</b>
          <span>Detected locally</span>
        </article>
        <article className="result">
          <BrandMark aria-hidden="true" decoding="async" loading="lazy" tone="ink" />
          <b>TouchGrassBar</b>
          <span>One quiet place</span>
        </article>
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
        <span>“It’s a harness problem.”</span>
        <span>“It’s not a loop. It’s orchestration.”</span>
        <span>“It’s a reasoning graph.”</span>
        <span>“Human in the loop. Eventually.”</span>
        <span>
          “<b className="d-diff-add">+24,982</b> <b className="d-diff-delete">−842</b>. LGTM.”
        </span>
        <span>“We reinvented while(true).”</span>
        <span>“The diff has a scrollbar.”</span>
        <span>“That’s good AI slop right there.”</span>
        <span>“Is it AGI yet?”</span>
        <span>“There’s a skill for that.”</span>
        <span>“I approve for a living.”</span>
        <span>“There’s an MCP for that.”</span>
        <span>“Glad I learned algorithms.”</span>
        <span>“10x engineer. $200/month.”</span>
        <span>“Who wrote this? Yes.”</span>
        <span>“CI is green again.”</span>
        <span>“The hallucination compiles.”</span>
        <span>“No AI was harmed writing this.”</span>
      </div>
      <header>
        <h2>
          Vibe code alone.
          <br />
          <em>Tokenmaxx together.</em>
        </h2>
        <p>Add your friends. Compare token usage. Keep every prompt private.</p>
        <a
          className="d-macos-download"
          data-analytics-event="download clicked"
          data-analytics-placement="invite"
          data-download-link
          href={downloadFallbackUrl}
        >
          <img alt="" src={appleLogo} />
          Download for macOS
        </a>
      </header>
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
      <span className="d-applications-folder" aria-hidden="true">
        <b>A</b>
      </span>
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
        <h2>
          <span>A tiny monument</span>
          <span>to your token problem.</span>
        </h2>
        <p>See how your Codex and Claude token usage compares with other Tokenmaxxers.</p>
        <a
          className="d-macos-download compact"
          data-analytics-event="download clicked"
          data-analytics-placement="release"
          data-download-link
          href={downloadFallbackUrl}
        >
          <img alt="" src={appleLogo} />
          Download for macOS
        </a>
      </div>
      <div className="d-release-install" aria-label="Install TouchGrassBar in Applications">
        <TouchGrassInstallItem />
        <span className="d-install-arrow" aria-hidden="true">
          →
        </span>
        <ApplicationsInstallItem />
      </div>
    </section>
  );
}

export default function LandingExperience({ initialGardenTime }: LandingExperienceProps) {
  const gardenTime = initialGardenTimeForRender(initialGardenTime);
  const heroCopy = GARDEN_COPY[gardenTime];

  return (
    <main className="direction direction-d identity-native" id="main-content">
      <header className="d-menubar">
        <div className="d-brand">
          <SiteBrand reversed />
        </div>
        <a
          className="d-header-download"
          data-analytics-event="download clicked"
          data-analytics-placement="header"
          data-download-link
          href={downloadFallbackUrl}
        >
          <img alt="" src={appleLogo} />
          Download for macOS
        </a>
      </header>

      <section
        className={`d-garden-hero garden-${gardenTime} d-time-instant`}
        suppressHydrationWarning
      >
        {(["dawn", "day", "golden", "night"] as const).map((time) => (
          <div
            className={`d-time-layer ${time === gardenTime ? "active" : ""} time-${time}`}
            data-garden-layer={time}
            key={time}
            suppressHydrationWarning
          />
        ))}
        <div className="d-mist-layer" />
        <div className="d-life-layer" aria-hidden="true">
          {Array.from({ length: 24 }, (_, index) => (
            <i className={index >= 14 ? "day-only" : undefined} key={index} />
          ))}
        </div>
        <div className="d-hero-inner">
          <div className="d-hero-copy">
            <span>Built for Codex & Claude</span>
            <h1>
              <span data-garden-copy-line="0" suppressHydrationWarning>
                {heroCopy[0]}
              </span>
              <br />
              <span data-garden-copy-line="1" suppressHydrationWarning>
                {heroCopy[1]}
              </span>
              <br />
              <em data-garden-copy-line="2" suppressHydrationWarning>
                {heroCopy[2]}
              </em>
            </h1>
            <p>
              Lives in your menu bar. See your limits and compare your usage on the leaderboard.
            </p>
            <a
              className="d-macos-download"
              data-analytics-event="download clicked"
              data-analytics-placement="hero"
              data-download-link
              href={downloadFallbackUrl}
            >
              <img alt="" src={appleLogo} />
              Download for macOS
            </a>
          </div>
          <ProductPanel />
        </div>
      </section>

      <section className="d-board-stage d-bar-only-stage" id="doomerboard">
        <header className="d-board-stage-copy">
          <h2>
            One board to rank them all.
            <br />
            <em>The Doomerboard.</em>
          </h2>
          <p>See who burned the most tokens and who still remembers daylight.</p>
        </header>
        <DoomerboardScoreboard />
      </section>

      <InviteSection />

      <SetupSection />

      <DownloadSection />

      <footer className="d-footer">
        <div className="d-footer-brand">
          <SiteBrand loading="lazy" reversed />
          <span>Open Source. Public usage. Private work.</span>
        </div>
        <nav aria-label="Project links">
          <a
            data-analytics-event="outbound link clicked"
            data-analytics-placement="github"
            href="https://github.com/FabienGreard/TouchGrassBar"
            rel="noreferrer"
            target="_blank"
          >
            <img alt="" src={githubLogo} />
            <span>Star on GitHub</span>
          </a>
          <a
            data-analytics-event="outbound link clicked"
            data-analytics-placement="x"
            href="https://x.com/FabienGreard"
            rel="noreferrer"
            target="_blank"
          >
            <img alt="" src={xLogo} />
            <span>@FabienGreard</span>
          </a>
        </nav>
      </footer>
    </main>
  );
}
