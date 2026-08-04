import type { SanitizedDesktopState } from "@touchgrass/contracts";

export type BrowserFixtureName =
  | "current"
  | "loading"
  | "stale"
  | "update"
  | "unavailable";

export function unavailableBrowserFixture(now = new Date()): unknown {
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

function populatedBrowserFixture(
  availability: "current" | "stale",
  now = new Date(),
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

export function browserFixture(
  name: BrowserFixtureName,
  now = new Date(),
): unknown {
  if (name === "current" || name === "update")
    return populatedBrowserFixture("current", now);
  if (name === "stale") return populatedBrowserFixture("stale", now);
  return unavailableBrowserFixture(now);
}

export function resolveBrowserFixtureName(search: string): BrowserFixtureName {
  const fixture = new URLSearchParams(search).get("fixture");
  return fixture === "current" ||
    fixture === "loading" ||
    fixture === "stale" ||
    fixture === "update"
    ? fixture
    : "unavailable";
}

export function acceptNewerSnapshot(
  current: SanitizedDesktopState | null,
  candidate: SanitizedDesktopState,
) {
  if (
    current !== null &&
    BigInt(candidate.revision) <= BigInt(current.revision)
  ) {
    return current;
  }
  return candidate;
}

export function shouldHidePanel(event: Pick<KeyboardEvent, "key" | "metaKey">) {
  return (
    event.key === "Escape" || (event.metaKey && event.key.toLowerCase() === "w")
  );
}
