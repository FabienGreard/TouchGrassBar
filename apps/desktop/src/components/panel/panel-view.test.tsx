import type { UsagePeriods } from "@touchgrass/contracts";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";

import { createBrowserSanitizedDesktopStateAdapter } from "@/dev/browser-sanitized-desktop-state-adapter";
import type { BrowserFixtureName } from "@/dev/preview-scenario";
import { Doomerboard, TokenmaxxersEmpty } from "@/components/panel/doomerboard";
import { PanelView } from "@/components/panel/panel-view";
import {
  currentProfile,
  currentDoomerboardRows,
  currentUsagePresentation,
  myTokenmaxxerRows,
} from "@/dev/panel-fixtures";
import { createSanitizedDesktopStateDelivery } from "@/native-state/sanitized-desktop-state-delivery";
import { createTauriSanitizedDesktopStateAdapter } from "@/native-state/tauri-sanitized-desktop-state-adapter";

function localDateTime(iso: string) {
  const parts = new Intl.DateTimeFormat("en-GB", {
    day: "numeric",
    hour: "2-digit",
    hourCycle: "h23",
    minute: "2-digit",
    month: "short",
    weekday: "short",
  }).formatToParts(new Date(iso));
  const part = (type: Intl.DateTimeFormatPartTypes) =>
    parts.find((candidate) => candidate.type === type)?.value ?? "";

  return `${part("weekday")} ${part("day")} ${part("month")}, ${part("hour")}:${part("minute")}`;
}

function localTime(iso: string) {
  return new Intl.DateTimeFormat("en-GB", {
    hour: "2-digit",
    hourCycle: "h23",
    minute: "2-digit",
  }).format(new Date(iso));
}

async function deliveredBrowserFixture(name: BrowserFixtureName) {
  const delivery = createSanitizedDesktopStateDelivery(
    createBrowserSanitizedDesktopStateAdapter(
      name,
      () => new Date("2026-08-06T13:45:00.000Z"),
    ),
  );

  let stop: (() => void) | undefined;
  const delivered = new Promise<void>((resolve, reject) => {
    stop = delivery.subscribe(() => {
      const view = delivery.getSnapshot();
      if (view.phase === "ready") resolve();
      if (view.phase === "degraded") reject(new Error("fixture unavailable"));
    });
  });
  await delivered;
  stop?.();

  const snapshot = delivery.getSnapshot().snapshot;
  if (snapshot === null) throw new Error("fixture unavailable");
  return snapshot;
}

async function waitForDeliveredRevision(
  delivery: ReturnType<typeof createSanitizedDesktopStateDelivery>,
  revision: string,
) {
  for (let attempt = 0; attempt < 50; attempt += 1) {
    if (delivery.getSnapshot().snapshot?.revision === revision) return;
    await Promise.resolve();
  }
  throw new Error(`revision ${revision} was not delivered`);
}

describe("panel states", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-08-06T13:45:00.000Z"));
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  test("presents the unavailable panel without fake data", async () => {
    const unavailableState = await deliveredBrowserFixture("unavailable");
    const markup = renderToStaticMarkup(
      <PanelView
        error={false}
        onRefresh={() => undefined}
        onSettings={() => undefined}
        refreshing={false}
        state={unavailableState}
      />,
    );

    expect(markup).not.toContain(">0<");
    expect(markup).toContain("Sync unavailable");
    expect(markup).toContain('aria-label="Open panel menu"');
    expect(markup).toContain('data-slot="brand-mark"');
    expect(markup).toContain('data-tone="ink"');
    expect(markup).toMatch(/<img[^>]*brightness-0[^>]*data-slot="brand-mark"/);
    expect(markup.match(/aria-label="Open panel menu"/g)).toHaveLength(1);
    expect(markup).toContain('data-icon-provider="hugeicons"');
    expect(markup).toContain("Leaderboard unavailable");
    expect(markup).not.toContain("Add by ID");
    expect(markup).toContain('aria-label="Current user profile unavailable"');
    expect(markup).not.toContain("— users");
    expect(markup).toContain('data-slot="doomerboard-viewport"');
    expect(markup).toContain("h-[180px]");
    expect(markup).toContain("Friends");
    expect(markup).toContain("Global");
    expect(markup).toMatch(
      /aria-label="Select Leaderboard period"[^>]*>Today<\/button>/,
    );
    expect(markup).toContain("Combined");
    expect(markup).toContain('aria-label="Select Leaderboard period"');
    expect(markup).toContain('aria-label="Select Leaderboard provider"');
    expect(markup.match(/aria-expanded:bg-pearl-ink\/5/g)).toHaveLength(3);
    expect(markup.match(/data-slot="metric-gauge"/g)).toHaveLength(3);
    expect(markup).not.toContain('data-slot="provider-quota-lane"');
    expect(markup.match(/data-slot="quota-progress"/g)).toHaveLength(2);
    expect(markup).not.toContain("Weekly limit");
    expect(markup).not.toContain("5-hour limit");
    expect(
      markup.match(/data-provider-availability="unavailable"/g),
    ).toHaveLength(2);
    expect(markup).not.toContain("1970-01-01");
    expect(markup).not.toMatch(/>(?:laura|nora|max)<|#TG-/i);
    expect(markup).not.toContain('data-slot="skeleton"');
    expect(markup).not.toContain("ml-0.5");
    expect(markup).not.toContain("lucide");
    expect(markup).not.toContain("translate-y-px");
    expect(markup).toContain('data-icon-provider="hugeicons"');
    expect(markup).toContain('data-icon-tone="default"');
    expect(markup.match(/min-w-\[82px\]/g)).toHaveLength(2);
    expect(markup.match(/px-\[13px\]/g)).toHaveLength(2);
    expect(markup).toContain("w-[402px]");
  });

  test("ships contrast and motion adaptations", () => {
    const markup = renderToStaticMarkup(
      <PanelView
        error={false}
        onRefresh={() => undefined}
        onSettings={() => undefined}
        refreshing={true}
        state={null}
      />,
    );

    expect(markup).toContain('data-slot="dropdown-menu-trigger"');
    expect(markup).toContain("Connecting…");
    expect(markup).toContain('data-slot="loading-panel"');
    expect(markup).toContain('aria-busy="true"');
    expect(markup).not.toContain("Weekly limit");
    expect(markup).not.toContain("5-hour limit");
    expect(
      markup.match(/data-provider-availability="unavailable"/g),
    ).toHaveLength(2);
    expect(markup).not.toContain("1970-01-01");
    expect(markup).toContain("Leaderboard unavailable");
    expect(markup).not.toContain('data-slot="provider-quota-lane"');
    expect(markup.match(/data-slot="quota-progress"/g)).toHaveLength(2);
    expect(markup).not.toContain('data-slot="skeleton"');
    expect(markup).toContain("contrast-more:border-pearl-ink");
    expect(markup).toContain("animate-pulse motion-reduce:animate-none");
  });

  test("does not expose internal snapshot diagnostics", () => {
    const markup = renderToStaticMarkup(
      <PanelView
        error
        onRefresh={() => undefined}
        onSettings={() => undefined}
        refreshing={false}
        state={null}
      />,
    );

    expect(markup).toContain("Sync unavailable");
    expect(markup).toContain("Local provider state unavailable");
    expect(markup).toContain('data-slot="loading-panel"');
    expect(markup).not.toContain('aria-busy="true"');
    expect(markup).not.toContain("animate-pulse");
    expect(markup).not.toContain("Nothing invented");
    expect(markup).not.toContain("native snapshot");
    expect(markup).not.toContain('role="alert"');
  });

  test("shows connecting before the first native snapshot", () => {
    const markup = renderToStaticMarkup(
      <PanelView
        error={false}
        onRefresh={() => undefined}
        onSettings={() => undefined}
        refreshing={false}
        state={null}
      />,
    );

    expect(markup).toContain("Connecting…");
    expect(markup).not.toContain("Live");
  });

  test("keeps the compact update icon when an update is available", async () => {
    const currentState = await deliveredBrowserFixture("current");
    const markup = renderToStaticMarkup(
      <PanelView
        error={false}
        onRefresh={() => undefined}
        onSettings={() => undefined}
        onUpdate={() => undefined}
        refreshing={false}
        state={currentState}
        updateState={{
          automaticChecksEnabled: true,
          contractVersion: 2,
          currentVersion: "1.3.2",
          onlineFeaturesPaused: false,
          update: {
            status: "available",
            version: "1.4.0",
          },
        }}
      />,
    );

    expect(markup).toContain('data-slot="update-action"');
    expect(markup).toContain('data-variant="primary"');
    expect(markup).toContain('aria-label="Install update and relaunch"');
    expect(markup).toContain('data-size="icon"');
    expect(markup).toContain('data-icon-source="Download04Icon"');
    expect(markup).not.toContain('data-slot="update-sheet"');

    const retryMarkup = renderToStaticMarkup(
      <PanelView
        error={false}
        onRefresh={() => undefined}
        onSettings={() => undefined}
        onUpdate={() => undefined}
        refreshing={false}
        state={currentState}
        updateState={{
          automaticChecksEnabled: true,
          contractVersion: 2,
          currentVersion: "1.3.2",
          onlineFeaturesPaused: true,
          update: {
            failure: "signature",
            status: "failed",
            version: "1.4.0",
          },
        }}
      />,
    );
    expect(retryMarkup).toContain('aria-label="Retry required update"');
    expect(retryMarkup).not.toContain(
      'aria-label="Install update and relaunch"',
    );
  });

  test("renders the populated development fixture without changing the production fallback", async () => {
    const currentState = await deliveredBrowserFixture("current");
    const markup = renderToStaticMarkup(
      <PanelView
        currentProfile={currentProfile}
        doomerboardRows={currentDoomerboardRows}
        error={false}
        onRefresh={() => undefined}
        onSettings={() => undefined}
        refreshing={false}
        state={currentState}
        usagePresentation={currentUsagePresentation}
      />,
    );

    expect(markup).not.toContain("data-preview-fixture");
    expect(markup).toContain("laura");
    expect(markup).toContain("18.2M");
    expect(markup).toContain("#TG-4COLD7");
    expect(markup).toContain("#TG-LOOP55");
    expect(markup).toContain("#TG-7K4P9D");
    expect(markup).toContain("Fabien#TG-7K4P9D");
    expect(markup).toContain(
      'aria-label="Copy current user profile Fabien#TG-7K4P9D"',
    );
    expect(markup).toContain('data-copy-status="idle"');
    expect(markup).not.toContain("data-copy-indicator");
    expect(markup).toContain('data-copy-feedback="idle"');
    expect(markup).not.toContain(">Copied<");
    expect(markup).toContain('data-slot="current-user-profile"');
    expect(markup).not.toContain('data-slot="panel-profile-footer"');
    expect(markup).not.toContain("2nd");
    expect(markup).toContain('data-slot="segmented-control"');
    expect(markup).not.toContain("8 users");
    expect(markup).toContain("-8%");
    expect(markup).toContain("Down 8 percent from the previous day");
    expect(markup).toContain('data-tone="negative"');
    expect(markup).toContain("text-destructive");
    expect(markup).toContain("+14%");
    expect(markup).toContain("+22%");
    expect(markup).toContain("width:34%");
    expect(markup).toContain("width:64%");
    expect(markup).toContain("width:100%");
    expect(markup).toContain(
      `Weekly limit · 4d 10h left · ${localDateTime("2026-08-10T23:45:00.000Z")}`,
    );
    expect(markup).toContain(
      `5-hour limit · resets ${localTime("2026-08-06T18:40:00.000Z")}`,
    );
    expect(markup).toContain(
      `Weekly limit · 6d 13h left · ${localDateTime("2026-08-13T03:00:00.000Z")}`,
    );
    expect(markup).toContain(
      `5-hour limit · resets ${localTime("2026-08-06T18:20:00.000Z")}`,
    );
    expect(markup).toContain('data-slot="provider-quota-lane"');
    expect(markup.match(/data-quota-tone="codex"/g)).toHaveLength(2);
    expect(markup.match(/data-quota-tone="claude"/g)).toHaveLength(2);
    expect(markup).toContain(
      'aria-label="Codex quota current, 74 percent remaining"',
    );
    expect(markup).toContain(
      'aria-label="5-hour limit quota current, 62 percent remaining"',
    );
    expect(markup).toContain('data-quota-value="74"');
    expect(markup).toContain('data-quota-value="18"');
    expect(markup).toContain("--quota-codex-low");
    expect(markup).toContain("--quota-claude-low");
    expect(markup).toContain('data-slot="scroll-area-root"');
    expect(markup).toContain('data-slot="scroll-area-viewport"');
    expect(markup).toContain('data-slot="doomerboard-viewport"');
    expect(markup).toContain('data-slot="doomerboard-ledger"');
    expect(markup).toContain("data-radix-scroll-area-viewport");
    expect(markup).not.toMatch(/data-doomerboard-scroll=""[^>]*tabindex/);
    expect(markup).not.toMatch(/data-slot="doomerboard-ledger"[^>]*tabindex/);
    expect(markup).not.toContain("Leaderboard unavailable");
    expect(markup).toContain("Friends");
  });

  test("renders the provider visibility selected by the native snapshot", async () => {
    const hiddenState = await deliveredBrowserFixture("current");
    hiddenState.providers = hiddenState.providers.filter(
      (provider) => provider.provider !== "claude",
    );
    const hiddenMarkup = renderToStaticMarkup(
      <PanelView
        error={false}
        onRefresh={() => undefined}
        onSettings={() => undefined}
        refreshing={false}
        state={hiddenState}
      />,
    );
    expect(hiddenMarkup).not.toContain('id="claude-heading"');

    const previousValueState = await deliveredBrowserFixture("current");
    const previousClaude = previousValueState.providers.find(
      (provider) => provider.provider === "claude",
    );
    if (!previousClaude) throw new Error("Claude fixture unavailable");
    previousClaude.presence = "not-detected";
    const previousValueMarkup = renderToStaticMarkup(
      <PanelView
        error={false}
        onRefresh={() => undefined}
        onSettings={() => undefined}
        refreshing={false}
        state={previousValueState}
      />,
    );
    expect(previousValueMarkup).toContain('id="claude-heading"');
  });

  test("updates Claude lanes after a native revision notice", async () => {
    const unavailableState = await deliveredBrowserFixture("unavailable");
    const currentState = await deliveredBrowserFixture("current");
    let nativeState = unavailableState;
    let emitRevision: ((event: { payload: unknown }) => void) | undefined;
    const delivery = createSanitizedDesktopStateDelivery(
      createTauriSanitizedDesktopStateAdapter({
        invoke: () => Promise.resolve(nativeState),
        listen: (_event, receive) => {
          emitRevision = receive;
          return Promise.resolve(() => undefined);
        },
        onFocusChanged: () => Promise.resolve(() => undefined),
      }),
    );
    const stop = delivery.subscribe(() => undefined);
    await waitForDeliveredRevision(delivery, "1");

    const unavailableMarkup = renderToStaticMarkup(
      <PanelView
        error={false}
        onRefresh={() => undefined}
        onSettings={() => undefined}
        refreshing={false}
        state={delivery.getSnapshot().snapshot}
      />,
    );
    expect(unavailableMarkup).not.toContain('data-quota-tone="claude"');

    nativeState = currentState;
    emitRevision?.({ payload: { revision: "2" } });
    await waitForDeliveredRevision(delivery, "2");
    const currentMarkup = renderToStaticMarkup(
      <PanelView
        error={false}
        onRefresh={() => undefined}
        onSettings={() => undefined}
        refreshing={false}
        state={delivery.getSnapshot().snapshot}
      />,
    );

    expect(currentMarkup.match(/data-quota-tone="claude"/g)).toHaveLength(2);
    expect(currentMarkup).toContain(
      'aria-label="Claude quota current, 18 percent remaining"',
    );
    expect(currentMarkup).toContain(
      'aria-label="5-hour limit quota current, 43 percent remaining"',
    );
    stop();
  });

  test("shows Claude-only observed usage in all accessible metric cards", async () => {
    const claudeOnlyState = structuredClone(
      await deliveredBrowserFixture("current"),
    );
    const unavailableUsage: UsagePeriods = {
      scanStatus: "unavailable",
      sevenDayScanStatus: "unavailable",
      sevenDays: { availability: "unavailable" },
      thirtyDayScanStatus: "unavailable",
      thirtyDays: { availability: "unavailable" },
      today: { availability: "unavailable" },
      todayScanStatus: "unavailable",
    };
    const observedAt = "2026-08-06T13:44:00.000Z";
    const pricingBasis = "anthropic-standard-2026-08-07-v1";
    const claudeUsage: UsagePeriods = {
      scanStatus: "complete",
      sevenDayScanStatus: "complete",
      sevenDays: {
        apiEquivalentCostBasis: pricingBasis,
        apiEquivalentCostCoveragePercent: null,
        apiEquivalentCostQuality: "local-only",
        apiEquivalentCostUsd: 24.5,
        availability: "current",
        coverage: "partial",
        evidenceBasis: "locally-derived",
        observedAt,
        observedTokens: 8_000_000,
        trendPercent: 25,
      },
      thirtyDayScanStatus: "complete",
      thirtyDays: {
        apiEquivalentCostBasis: pricingBasis,
        apiEquivalentCostCoveragePercent: null,
        apiEquivalentCostQuality: "local-only",
        apiEquivalentCostUsd: 61.75,
        availability: "current",
        coverage: "partial",
        evidenceBasis: "locally-derived",
        observedAt,
        observedTokens: 20_000_000,
        trendPercent: 0,
      },
      today: {
        apiEquivalentCostBasis: pricingBasis,
        apiEquivalentCostCoveragePercent: null,
        apiEquivalentCostQuality: "local-only",
        apiEquivalentCostUsd: 6.25,
        availability: "current",
        coverage: "partial",
        evidenceBasis: "locally-derived",
        observedAt,
        observedTokens: 2_000_000,
        trendPercent: -12.5,
      },
      todayScanStatus: "complete",
    };
    const codex = claudeOnlyState.providers.find(
      (provider) => provider.provider === "codex",
    );
    const claude = claudeOnlyState.providers.find(
      (provider) => provider.provider === "claude",
    );
    if (!codex || !claude) throw new Error("provider fixture unavailable");
    codex.usage = structuredClone(unavailableUsage);
    claude.usage = structuredClone(claudeUsage);
    claudeOnlyState.combinedUsage = structuredClone(claudeUsage);

    const markup = renderToStaticMarkup(
      <PanelView
        error={false}
        onRefresh={() => undefined}
        onSettings={() => undefined}
        refreshing={false}
        state={claudeOnlyState}
      />,
    );

    expect(markup).toContain('data-slot="metric-card-label">Today');
    expect(markup).toContain('data-slot="metric-card-label">7 days');
    expect(markup).toContain('data-slot="metric-card-label">30 days');
    expect(markup).toContain(">2M</strong>");
    expect(markup).toContain(">8M</strong>");
    expect(markup).toContain(">20M</strong>");
    expect(markup).toContain(
      'aria-label="Down 12.5 percent from the previous day"',
    );
    expect(markup).toContain(
      'aria-label="Up 25 percent from the previous 7 days"',
    );
    expect(markup).toContain(
      'aria-label="No change from the previous 30 days"',
    );
    expect(markup).toContain(
      `aria-label="≈ $6.25, locally observed token evidence, partial period coverage, cost estimated from local pricing evidence, pricing basis ${pricingBasis}"`,
    );
    expect(markup).toContain(
      `aria-label="≈ $24.50, locally observed token evidence, partial period coverage, cost estimated from local pricing evidence, pricing basis ${pricingBasis}"`,
    );
    expect(markup).toContain(
      `aria-label="≈ $61.75, locally observed token evidence, partial period coverage, cost estimated from local pricing evidence, pricing basis ${pricingBasis}"`,
    );
    expect(markup).toContain('aria-label="Today usage gauge 10 percent"');
    expect(markup).toContain('aria-label="7 days usage gauge 40 percent"');
    expect(markup).toContain('aria-label="30 days usage gauge 100 percent"');
    expect(markup).toContain("width:10%");
    expect(markup).toContain("width:40%");
    expect(markup).toContain("width:100%");
    expect(markup).not.toContain("≈ $38.61");
  });

  test("shows compact costs without exposing internal quality labels", async () => {
    const currentState = await deliveredBrowserFixture("current");
    const markup = renderToStaticMarkup(
      <PanelView
        error={false}
        onRefresh={() => undefined}
        onSettings={() => undefined}
        refreshing={false}
        state={currentState}
      />,
    );

    expect(markup).toContain("-8.6%");
    expect(markup).toContain("Down 8.6 percent from the previous day");
    expect(markup).toContain("+15%");
    expect(markup).toContain("+20.3%");
    expect(markup).toContain("≈ $44.86");
    expect(markup).toContain("≈ $239.46");
    expect(markup).toContain("≈ $918.48");
    expect(markup).not.toContain(">Reconciled<");
    expect(markup).not.toContain(">Modeled");
    expect(markup).not.toContain(">Local only<");
    expect(markup).toContain("mixed token evidence");
    expect(markup).toContain("partial period coverage");
    expect(markup).toContain("cost estimated from local pricing evidence");
    expect(markup).toContain(
      "pricing basis openai-api-2026-08-09-v2 + anthropic-standard-2026-08-07-v1",
    );
    expect(markup).toContain("width:5%");
    expect(markup).toContain("width:26%");
    expect(markup).toContain("width:100%");
  });

  test("shows a compact indexing state when cost evidence is not ready", async () => {
    const currentState = await deliveredBrowserFixture("current");
    currentState.combinedUsage.scanStatus = "indexing";
    currentState.combinedUsage.todayScanStatus = "indexing";
    const today = currentState.combinedUsage.today;
    if (today.availability === "unavailable")
      throw new Error("fixture unavailable");
    today.apiEquivalentCostUsd = null;
    today.apiEquivalentCostBasis = null;
    today.apiEquivalentCostQuality = null;
    today.apiEquivalentCostCoveragePercent = null;
    const sevenDays = currentState.combinedUsage.sevenDays;
    const thirtyDays = currentState.combinedUsage.thirtyDays;
    if (
      sevenDays.availability === "unavailable" ||
      thirtyDays.availability === "unavailable"
    )
      throw new Error("fixture unavailable");
    sevenDays.apiEquivalentCostQuality = "modeled";
    sevenDays.apiEquivalentCostCoveragePercent = 80;
    thirtyDays.apiEquivalentCostQuality = "local-only";
    thirtyDays.apiEquivalentCostCoveragePercent = null;
    const markup = renderToStaticMarkup(
      <PanelView
        error={false}
        onRefresh={() => undefined}
        onSettings={() => undefined}
        refreshing={false}
        state={currentState}
      />,
    );

    expect(markup).toContain("Indexing…");
    expect(markup).toContain("≈ $239.46");
    expect(markup).toContain("≈ $918.48");
    expect(markup).not.toContain('data-icon-spin="true"');
    expect(markup).not.toContain("Finish now");
    expect(markup).not.toContain("API equivalent unavailable");
    expect(markup).not.toContain(">Modeled");
    expect(markup).not.toContain(">Local only<");
    expect(markup).toContain("cost modeled from 80 percent priced evidence");
    expect(markup).toContain("cost estimated from local pricing evidence");
  });

  test("finishes recent cost periods before older periods", async () => {
    const currentState = await deliveredBrowserFixture("current");
    currentState.combinedUsage.scanStatus = "indexing";
    currentState.combinedUsage.todayScanStatus = "complete";
    currentState.combinedUsage.sevenDayScanStatus = "indexing";
    currentState.combinedUsage.thirtyDayScanStatus = "indexing";
    for (const total of [
      currentState.combinedUsage.today,
      currentState.combinedUsage.sevenDays,
      currentState.combinedUsage.thirtyDays,
    ]) {
      if (total.availability === "unavailable") continue;
      total.apiEquivalentCostUsd = null;
      total.apiEquivalentCostBasis = null;
      total.apiEquivalentCostQuality = null;
      total.apiEquivalentCostCoveragePercent = null;
    }

    const markup = renderToStaticMarkup(
      <PanelView
        error={false}
        onRefresh={() => undefined}
        onSettings={() => undefined}
        refreshing={false}
        state={currentState}
      />,
    );

    expect(markup.match(/Indexing…/g)).toHaveLength(2);
    expect(markup).toContain('aria-label="API equivalent not ready,');
  });

  test("announces stale Quota Lanes without hiding their last valid values", async () => {
    const staleState = await deliveredBrowserFixture("stale");
    const markup = renderToStaticMarkup(
      <PanelView
        error={false}
        onRefresh={() => undefined}
        onSettings={() => undefined}
        refreshing={false}
        state={staleState}
      />,
    );

    expect(markup).toContain('data-provider-availability="stale"');
    expect(markup).toContain(
      'aria-label="Codex quota stale, 74 percent remaining"',
    );
    expect(markup).toContain(
      'aria-label="5-hour limit quota stale, 62 percent remaining"',
    );
    expect(markup).toContain(
      `Weekly limit · 4d 10h left · ${localDateTime("2026-08-10T23:45:00.000Z")} · stale`,
    );
    expect(markup).toContain(
      `5-hour limit · resets ${localTime("2026-08-06T18:40:00.000Z")} · stale`,
    );
    expect(markup).toContain('data-quota-value="74"');
    expect(markup).toContain('data-quota-value="62"');
  });

  test("offers an honest invitation state for an empty Tokenmaxxers board", () => {
    const markup = renderToStaticMarkup(<TokenmaxxersEmpty />);

    expect(markup).toContain("Your Leaderboard is lonely");
    expect(markup).toContain(
      "Add Tokenmaxxers by TouchGrass ID to compare scores.",
    );
    expect(markup).toContain("Add a Tokenmaxxer");
    expect(markup).toContain('data-icon-provider="hugeicons"');
    expect(markup).toContain('data-icon-tone="default"');
    expect(markup).not.toContain("lucide");
    expect(markup).toContain("h-full");
    expect(markup).not.toMatch(/<button[^>]*\sdisabled(?:=|>)/);
  });

  test("offers a populated fake Tokenmaxxers development mockup", () => {
    const markup = renderToStaticMarkup(
      <Doomerboard
        currentProfile={currentProfile}
        initialAudience="mine"
        rows={currentDoomerboardRows}
        tokenmaxxerRows={myTokenmaxxerRows}
      />,
    );

    expect(markup).toContain('aria-label="My Tokenmaxxers rankings"');
    expect(markup).toContain("TOUCH GRASS?");
    expect(markup).toContain("STILL ONLINE");
    expect(markup).toContain("Fabien");
    expect(markup).not.toContain("Your Doomerboard is lonely");
  });

  test("does not infer the current Profile from fixture conventions", () => {
    const markup = renderToStaticMarkup(
      <Doomerboard rows={currentDoomerboardRows} />,
    );

    expect(markup).toContain('aria-label="Current user profile unavailable"');
    expect(markup).not.toContain("Copy current user profile");
  });

  test("keeps every Tokenmaxxer when Token Scores share a rank", () => {
    const tiedRows = [
      {
        displayName: "ada",
        rank: 1,
        tokenScore: "18.2M",
        touchGrassId: "#TG-AAAAAA",
      },
      {
        displayName: "bea",
        rank: 1,
        tokenScore: "18.2M",
        touchGrassId: "#TG-BBBBBB",
      },
      {
        displayName: "max",
        rank: 3,
        tokenScore: "9.1M",
        touchGrassId: "#TG-CCCCCC",
      },
    ];
    const markup = renderToStaticMarkup(<Doomerboard rows={tiedRows} />);

    expect(markup).toContain("ada");
    expect(markup).toContain("bea");
    expect(markup).toContain("#TG-AAAAAA");
    expect(markup).toContain("#TG-BBBBBB");
  });
});
