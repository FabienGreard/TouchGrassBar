import type { SanitizedDesktopStatePort } from "@/native-state/sanitized-desktop-state-delivery";
import type { BrowserFixtureName } from "@/dev/preview-scenario";

function unavailableFixture(now: Date): unknown {
  return {
    contractVersion: 3,
    generatedAt: now.toISOString(),
    profile: { status: "not-authorized" },
    providers: [
      { availability: "unavailable", provider: "codex", quotaLanes: [] },
      { availability: "unavailable", provider: "claude", quotaLanes: [] },
    ],
    revision: "1",
    sync: { lastSuccessfulAt: null, status: "unavailable" },
    usage: {
      claude: {
        scanStatus: "unavailable",
        sevenDays: { availability: "unavailable" },
        thirtyDays: { availability: "unavailable" },
        today: { availability: "unavailable" },
      },
      codex: {
        scanStatus: "unavailable",
        sevenDays: { availability: "unavailable" },
        thirtyDays: { availability: "unavailable" },
        today: { availability: "unavailable" },
      },
    },
  };
}

function observedUsage(
  availability: "current" | "stale",
  observedAt: string,
  observedTokens: number,
  apiEquivalentCostUsd: number,
  trendPercent: number,
) {
  return {
    apiEquivalentCostBasis: "openai-standard-2026-08-06-v1",
    apiEquivalentCostCoveragePercent: null,
    apiEquivalentCostQuality: "reconciled",
    apiEquivalentCostUsd,
    availability,
    coverage: "complete",
    evidenceBasis: "provider-reported",
    observedAt,
    observedTokens,
    trendPercent,
  };
}

function populatedFixture(
  availability: "current" | "stale",
  now: Date,
): unknown {
  const observedAt = now.toISOString();
  const resetAfter = (minutes: number) =>
    new Date(now.getTime() + minutes * 60_000).toISOString();

  return {
    contractVersion: 3,
    generatedAt: observedAt,
    profile: {
      displayName: "Fabien",
      status: "ready",
      touchGrassId: "TG-7K4P9D",
    },
    providers: [
      {
        availability,
        observedAt,
        provider: "codex",
        quotaLanes: [
          {
            allowance: 100,
            label: "Weekly limit",
            remaining: 74,
            resetAt: resetAfter((4 * 24 + 10) * 60),
            unit: "percent",
          },
          {
            allowance: 100,
            label: "5-hour limit",
            remaining: 62,
            resetAt: resetAfter(4 * 60 + 55),
            unit: "percent",
          },
        ],
      },
      {
        availability,
        observedAt,
        provider: "claude",
        quotaLanes: [
          {
            allowance: 100,
            label: "Weekly limit",
            remaining: 18,
            resetAt: resetAfter((6 * 24 + 13) * 60 + 15),
            unit: "percent",
          },
          {
            allowance: 100,
            label: "5-hour limit",
            remaining: 43,
            resetAt: resetAfter(4 * 60 + 35),
            unit: "percent",
          },
        ],
      },
    ],
    revision: availability === "current" ? "2" : "3",
    sync: {
      lastSuccessfulAt: observedAt,
      status: availability === "current" ? "synced" : "stale",
    },
    usage: {
      claude: {
        scanStatus: "unavailable",
        sevenDays: { availability: "unavailable" },
        thirtyDays: { availability: "unavailable" },
        today: { availability: "unavailable" },
      },
      codex: {
        scanStatus: "complete",
        sevenDays: observedUsage(
          availability,
          observedAt,
          71_400_000,
          214.96,
          14,
        ),
        thirtyDays: observedUsage(
          availability,
          observedAt,
          284_600_000,
          856.73,
          22,
        ),
        today: observedUsage(
          availability,
          observedAt,
          12_800_000,
          38.61,
          -8,
        ),
      },
    },
  };
}

function fixture(name: BrowserFixtureName, now: Date): unknown {
  if (name === "current" || name === "update")
    return populatedFixture("current", now);
  if (name === "stale") return populatedFixture("stale", now);
  return unavailableFixture(now);
}

export function createBrowserSanitizedDesktopStateAdapter(
  name: BrowserFixtureName,
  now = () => new Date(),
): SanitizedDesktopStatePort {
  const snapshot = fixture(name, now());

  return {
    readSnapshot: () =>
      name === "loading"
        ? new Promise<never>(() => undefined)
        : Promise.resolve({ ok: true, value: snapshot } as const),
    requestRefresh: () =>
      Promise.resolve({ ok: true, value: { accepted: true } } as const),
    subscribeToInvalidations: () =>
      Promise.resolve({ ok: true, value: () => undefined } as const),
  };
}
