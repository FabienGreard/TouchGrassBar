import type { SanitizedDesktopStatePort } from "@/native-state/sanitized-desktop-state-delivery";
import type { BrowserFixtureName } from "@/dev/preview-scenario";

function unavailableFixture(now: Date): unknown {
  return {
    contractVersion: 1,
    generatedAt: now.toISOString(),
    providers: [
      { availability: "unavailable", provider: "codex", quotaLanes: [] },
      { availability: "unavailable", provider: "claude", quotaLanes: [] },
    ],
    revision: "1",
    sync: { lastSuccessfulAt: null, status: "unavailable" },
    usage: {
      claude: {
        sevenDays: { availability: "unavailable" },
        thirtyDays: { availability: "unavailable" },
        today: { availability: "unavailable" },
      },
      codex: {
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
) {
  return {
    apiEquivalentCostUsd,
    availability,
    coverage: "complete",
    evidenceBasis: "provider-reported",
    observedAt,
    observedTokens,
  };
}

function populatedFixture(
  availability: "current" | "stale",
  now: Date,
): unknown {
  const observedAt = now.toISOString();

  return {
    contractVersion: 1,
    generatedAt: observedAt,
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
            resetAt: "2026-08-03T08:00:00.000Z",
            unit: "percent",
          },
          {
            allowance: 100,
            label: "5-hour limit",
            remaining: 62,
            resetAt: "2026-08-03T14:40:00.000Z",
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
            resetAt: "2026-08-06T03:00:00.000Z",
            unit: "percent",
          },
          {
            allowance: 100,
            label: "5-hour limit",
            remaining: 43,
            resetAt: "2026-08-03T18:20:00.000Z",
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
        sevenDays: { availability: "unavailable" },
        thirtyDays: { availability: "unavailable" },
        today: { availability: "unavailable" },
      },
      codex: {
        sevenDays: observedUsage(availability, observedAt, 71_400_000, 214.96),
        thirtyDays: observedUsage(
          availability,
          observedAt,
          284_600_000,
          856.73,
        ),
        today: observedUsage(availability, observedAt, 12_800_000, 38.61),
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
