import type {
  CodingProvider,
  SanitizedDesktopState,
  UsageCoverage,
  UsageEvidenceBasis,
  UsagePeriods,
  UsageScanStatus,
  UsageTotal,
} from "@touchgrass/contracts";

import type { SanitizedDesktopStatePort } from "@/native-state/sanitized-desktop-state-delivery";
import type { BrowserFixtureName } from "@/dev/preview-scenario";

type ProviderEnablement = Readonly<Record<CodingProvider, boolean>>;
type ObservedUsageTotal = Exclude<
  UsageTotal,
  { availability: "unavailable" }
>;

const allProvidersEnabled: ProviderEnablement = {
  claude: true,
  codex: true,
};

function unavailableUsage(): UsagePeriods {
  return {
    scanStatus: "unavailable",
    sevenDayScanStatus: "unavailable",
    sevenDays: { availability: "unavailable" },
    thirtyDayScanStatus: "unavailable",
    thirtyDays: { availability: "unavailable" },
    today: { availability: "unavailable" },
    todayScanStatus: "unavailable",
  };
}

function unavailableFixture(now: Date): SanitizedDesktopState {
  return {
    combinedUsage: unavailableUsage(),
    contractVersion: 3,
    generatedAt: now.toISOString(),
    profile: { status: "not-authorized" },
    providers: [
      {
        displayName: "Codex",
        presence: "detected",
        provider: "codex",
        quota: { availability: "unavailable", provider: "codex", quotaLanes: [] },
        usage: unavailableUsage(),
      },
      {
        displayName: "Claude",
        presence: "detected",
        provider: "claude",
        quota: { availability: "unavailable", provider: "claude", quotaLanes: [] },
        usage: unavailableUsage(),
      },
    ],
    revision: "1",
    sync: { lastSuccessfulAt: null, status: "unavailable" },
  };
}

function observedUsage(
  availability: "current" | "stale",
  observedAt: string,
  observedTokens: number,
  apiEquivalentCostUsd: number,
  trendPercent: number,
  options: {
    apiEquivalentCostBasis: string;
    apiEquivalentCostQuality: "local-only" | "reconciled";
    coverage: UsageCoverage;
    evidenceBasis: UsageEvidenceBasis;
  },
): ObservedUsageTotal {
  const previousPeriodRatio = 1 + trendPercent / 100;
  return {
    apiEquivalentCostBasis: options.apiEquivalentCostBasis,
    apiEquivalentCostCoveragePercent: null,
    apiEquivalentCostQuality: options.apiEquivalentCostQuality,
    apiEquivalentCostUsd,
    availability,
    coverage: options.coverage,
    evidenceBasis: options.evidenceBasis,
    observedAt,
    observedTokens,
    trendPercent,
    trendPreviousTokens:
      previousPeriodRatio > 0
        ? Math.round(observedTokens / previousPeriodRatio)
        : null,
  };
}

function isObservedUsage(total: UsageTotal): total is ObservedUsageTotal {
  return total.availability !== "unavailable";
}

function combinedUsageTotal(totals: readonly UsageTotal[]): UsageTotal {
  const observed = totals.filter(isObservedUsage);
  const firstObserved = observed[0];
  if (firstObserved === undefined) return { availability: "unavailable" };
  if (observed.length === 1) return { ...firstObserved };

  const observedTokens = observed.reduce(
    (total, usage) => total + usage.observedTokens,
    0,
  );
  const hasCompleteTrendEvidence = observed.every(
    (usage) =>
      usage.trendPreviousTokens !== null &&
      usage.trendPreviousTokens !== undefined,
  );
  const trendPreviousTokens = hasCompleteTrendEvidence
    ? observed.reduce(
        (total, usage) => total + (usage.trendPreviousTokens ?? 0),
        0,
      )
    : null;
  const trendPercent =
    trendPreviousTokens === null || trendPreviousTokens === 0
      ? null
      : ((observedTokens - trendPreviousTokens) / trendPreviousTokens) * 100;
  const evidenceBases = new Set(observed.map((usage) => usage.evidenceBasis));
  const fixedQualityCosts = observed.every(
    (usage) =>
      usage.apiEquivalentCostUsd !== null &&
      usage.apiEquivalentCostUsd !== undefined &&
      usage.apiEquivalentCostBasis !== null &&
      usage.apiEquivalentCostBasis !== undefined &&
      (usage.apiEquivalentCostQuality === "reconciled" ||
        usage.apiEquivalentCostQuality === "local-only"),
  );
  const costFields = fixedQualityCosts
    ? {
        apiEquivalentCostBasis: [
          ...new Set(observed.map((usage) => usage.apiEquivalentCostBasis)),
        ].join(" + "),
        apiEquivalentCostCoveragePercent: null,
        apiEquivalentCostQuality: observed.some(
          (usage) => usage.apiEquivalentCostQuality === "local-only",
        )
          ? ("local-only" as const)
          : ("reconciled" as const),
        apiEquivalentCostUsd: observed.reduce(
          (total, usage) => total + (usage.apiEquivalentCostUsd ?? 0),
          0,
        ),
      }
    : {
        apiEquivalentCostBasis: null,
        apiEquivalentCostCoveragePercent: null,
        apiEquivalentCostQuality: null,
        apiEquivalentCostUsd: null,
      };

  return {
    ...costFields,
    availability: observed.some((usage) => usage.availability === "stale")
      ? "stale"
      : "current",
    coverage: observed.every((usage) => usage.coverage === "complete")
      ? "complete"
      : "partial",
    evidenceBasis:
      evidenceBases.size === 1 ? firstObserved.evidenceBasis : "mixed",
    observedAt: observed.reduce(
      (latest, usage) => (usage.observedAt > latest ? usage.observedAt : latest),
      firstObserved.observedAt,
    ),
    observedTokens,
    trendPercent,
    trendPreviousTokens,
  };
}

function combinedScanStatus(
  statuses: readonly UsageScanStatus[],
): UsageScanStatus {
  if (statuses.includes("indexing")) return "indexing";
  if (statuses.includes("complete")) return "complete";
  return "unavailable";
}

function combinedUsage(periods: readonly UsagePeriods[]): UsagePeriods {
  if (periods.length === 0) return unavailableUsage();

  return {
    scanStatus: combinedScanStatus(periods.map((usage) => usage.scanStatus)),
    sevenDayScanStatus: combinedScanStatus(
      periods.map((usage) => usage.sevenDayScanStatus ?? usage.scanStatus),
    ),
    sevenDays: combinedUsageTotal(periods.map((usage) => usage.sevenDays)),
    thirtyDayScanStatus: combinedScanStatus(
      periods.map((usage) => usage.thirtyDayScanStatus ?? usage.scanStatus),
    ),
    thirtyDays: combinedUsageTotal(periods.map((usage) => usage.thirtyDays)),
    today: combinedUsageTotal(periods.map((usage) => usage.today)),
    todayScanStatus: combinedScanStatus(
      periods.map((usage) => usage.todayScanStatus ?? usage.scanStatus),
    ),
  };
}

function projectProviderEnablement(
  state: SanitizedDesktopState,
  providerEnablement: ProviderEnablement,
): SanitizedDesktopState {
  const includedUsage: UsagePeriods[] = [];
  for (const presentation of state.providers) {
    if (providerEnablement[presentation.provider]) {
      includedUsage.push(presentation.usage);
    }
  }

  return {
    ...state,
    combinedUsage: combinedUsage(includedUsage),
    providers: state.providers.map((presentation) =>
      providerEnablement[presentation.provider]
        ? presentation
        : {
            ...presentation,
            quota: {
              availability: "unavailable",
              provider: presentation.provider,
              quotaLanes: [],
            },
            usage: unavailableUsage(),
          },
    ),
  };
}

function populatedFixture(
  availability: "current" | "stale",
  now: Date,
): SanitizedDesktopState {
  const observedAt = now.toISOString();
  const resetAfter = (minutes: number) =>
    new Date(now.getTime() + minutes * 60_000).toISOString();
  const codexUsage = {
    scanStatus: "complete",
    sevenDayScanStatus: "complete",
    sevenDays: observedUsage(
      availability,
      observedAt,
      71_400_000,
      214.96,
      14,
      {
        apiEquivalentCostBasis: "openai-standard-2026-08-06-v1",
        apiEquivalentCostQuality: "reconciled",
        coverage: "complete",
        evidenceBasis: "provider-reported",
      },
    ),
    thirtyDayScanStatus: "complete",
    thirtyDays: observedUsage(
      availability,
      observedAt,
      284_600_000,
      856.73,
      22,
      {
        apiEquivalentCostBasis: "openai-standard-2026-08-06-v1",
        apiEquivalentCostQuality: "reconciled",
        coverage: "complete",
        evidenceBasis: "provider-reported",
      },
    ),
    today: observedUsage(
      availability,
      observedAt,
      12_800_000,
      38.61,
      -8,
      {
        apiEquivalentCostBasis: "openai-standard-2026-08-06-v1",
        apiEquivalentCostQuality: "reconciled",
        coverage: "complete",
        evidenceBasis: "provider-reported",
      },
    ),
    todayScanStatus: "complete",
  } satisfies UsagePeriods;
  const claudeUsage = {
    scanStatus: "complete",
    sevenDayScanStatus: "complete",
    sevenDays: observedUsage(
      availability,
      observedAt,
      8_000_000,
      24.5,
      25,
      {
        apiEquivalentCostBasis: "anthropic-standard-2026-08-07-v1",
        apiEquivalentCostQuality: "local-only",
        coverage: "partial",
        evidenceBasis: "locally-derived",
      },
    ),
    thirtyDayScanStatus: "complete",
    thirtyDays: observedUsage(
      availability,
      observedAt,
      20_000_000,
      61.75,
      0,
      {
        apiEquivalentCostBasis: "anthropic-standard-2026-08-07-v1",
        apiEquivalentCostQuality: "local-only",
        coverage: "partial",
        evidenceBasis: "locally-derived",
      },
    ),
    today: observedUsage(
      availability,
      observedAt,
      2_000_000,
      6.25,
      -12.5,
      {
        apiEquivalentCostBasis: "anthropic-standard-2026-08-07-v1",
        apiEquivalentCostQuality: "local-only",
        coverage: "partial",
        evidenceBasis: "locally-derived",
      },
    ),
    todayScanStatus: "complete",
  } satisfies UsagePeriods;

  return {
    combinedUsage: combinedUsage([codexUsage, claudeUsage]),
    contractVersion: 3,
    generatedAt: observedAt,
    profile: {
      displayName: "Fabien",
      status: "ready",
      touchGrassId: "TG-7K4P9D",
    },
    providers: [
      {
        displayName: "Codex",
        presence: "detected",
        provider: "codex",
        quota: {
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
        usage: codexUsage,
      },
      {
        displayName: "Claude",
        presence: "detected",
        provider: "claude",
        quota: {
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
        usage: claudeUsage,
      },
    ],
    revision: availability === "current" ? "2" : "3",
    sync: {
      lastSuccessfulAt: observedAt,
      status: availability === "current" ? "synced" : "stale",
    },
  };
}

function fixture(name: BrowserFixtureName, now: Date): SanitizedDesktopState {
  if (name === "current" || name === "update")
    return populatedFixture("current", now);
  if (name === "stale") return populatedFixture("stale", now);
  return unavailableFixture(now);
}

export function createBrowserSanitizedDesktopStateAdapter(
  name: BrowserFixtureName,
  now = () => new Date(),
  providerEnablement: ProviderEnablement = allProvidersEnabled,
): SanitizedDesktopStatePort {
  const snapshot = projectProviderEnablement(
    fixture(name, now()),
    providerEnablement,
  );

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
