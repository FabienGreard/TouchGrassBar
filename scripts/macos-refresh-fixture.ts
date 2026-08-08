import { createHash } from "node:crypto";
import { writeFileSync } from "node:fs";

import {
  REFRESH_FIXTURE_BYTES,
  REFRESH_FIXTURE_SHA256,
  REFRESH_FIXTURE_VERSION,
} from "./macos-release-gates-contract";

export { REFRESH_FIXTURE_BYTES, REFRESH_FIXTURE_SHA256, REFRESH_FIXTURE_VERSION };

const FIXTURE_AS_OF_TIMESTAMP = Date.UTC(2026, 7, 8);
const MILLISECONDS_PER_DAY = 24 * 60 * 60 * 1_000;
const PUBLIC_ID_ALPHABET = "ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
const SUPPORTED_PROVIDERS = ["codex", "claude"] as const;

type SupportedProvider = (typeof SUPPORTED_PROVIDERS)[number];

type RankingDayUsageRecord = {
  ranking_day: string;
  revision: number;
  observed_tokens: number;
};

type ModelCostDay = {
  ranking_day: string;
  model: string;
  observed_tokens: number;
  api_equivalent_cost_micros: number;
  coverage: "complete";
  price_basis_version: "synthetic-pricing-v1";
};

type ProviderFixture = {
  provider: SupportedProvider;
  ranking_days: RankingDayUsageRecord[];
  model_cost_days: ModelCostDay[];
};

type DoomerboardRow = {
  apiEquivalentCostMicros: number;
  displayName: string;
  rank: number;
  tokenScore: number;
  touchGrassId: string;
};

type PanelProjection = {
  contractVersion: 3;
  generatedAt: string;
  revision: "1";
  providers: Array<Record<string, unknown>>;
  combinedUsage: Record<string, unknown>;
  sync: {
    status: "unavailable";
    lastSuccessfulAt: null;
  };
  profile: { status: "not-authorized" };
};

export type MacosRefreshFixture = {
  version: typeof REFRESH_FIXTURE_VERSION;
  source: "synthetic";
  maxima: {
    global_rows: 100;
    model_cost_days_per_provider: 30;
    my_tokenmaxxers_rows: 100;
    ranking_days_per_provider: 60;
    supported_providers: 2;
  };
  providers: ProviderFixture[];
  doomerboards: {
    global: DoomerboardRow[];
    my_tokenmaxxers: DoomerboardRow[];
  };
  panel_projection: PanelProjection;
};

export type MacosRefreshFixtureBinding = {
  version: typeof REFRESH_FIXTURE_VERSION;
  sha256: string;
  bytes: number;
};

export type MacosRefreshFixtureWriter = (outputFile: string, bytes: Uint8Array) => void;

function rankingDay(offset: number) {
  return new Date(FIXTURE_AS_OF_TIMESTAMP - offset * MILLISECONDS_PER_DAY)
    .toISOString()
    .slice(0, 10);
}

function providerFixture(provider: SupportedProvider, providerIndex: number): ProviderFixture {
  const rankingDays = Array.from({ length: 60 }, (_, offset) => ({
    ranking_day: rankingDay(offset),
    revision: 60 - offset,
    observed_tokens: 900_000_000 + providerIndex * 100_000_000 - offset * 1_000_000,
  }));

  return {
    provider,
    ranking_days: rankingDays,
    model_cost_days: rankingDays.slice(0, 30).map((day, offset) => ({
      ranking_day: day.ranking_day,
      model: `synthetic-${provider}-model-${String((offset % 3) + 1).padStart(2, "0")}`,
      observed_tokens: day.observed_tokens,
      api_equivalent_cost_micros: 2_000_000_000 + providerIndex * 100_000_000 - offset * 1_000_000,
      coverage: "complete",
      price_basis_version: "synthetic-pricing-v1",
    })),
  };
}

function syntheticTouchGrassId(index: number) {
  let value = index;
  let suffix = "";
  for (let position = 0; position < 6; position += 1) {
    suffix = PUBLIC_ID_ALPHABET[value % PUBLIC_ID_ALPHABET.length] + suffix;
    value = Math.floor(value / PUBLIC_ID_ALPHABET.length);
  }
  return `TG-${suffix}`;
}

function displayName(audience: "Global" | "My Tokenmaxxers", index: number) {
  return `Synthetic ${audience} ${String(index).padStart(3, "0")} `.padEnd(40, "X").slice(0, 40);
}

function doomerboardRows(audience: "Global" | "My Tokenmaxxers") {
  return Array.from({ length: 100 }, (_, offset): DoomerboardRow => {
    const rank = offset + 1;
    const tokenScore = 1_000_000_000 - offset * 1_000_000;
    return {
      apiEquivalentCostMicros: tokenScore * 2,
      displayName: displayName(audience, rank),
      rank,
      tokenScore,
      touchGrassId: syntheticTouchGrassId(rank),
    };
  });
}

function usageTotal(observedTokens: number, apiEquivalentCostMicros: number) {
  return {
    availability: "current",
    evidenceBasis: "locally-derived",
    coverage: "complete",
    observedAt: "2026-08-08T00:00:00Z",
    observedTokens,
    apiEquivalentCostUsd: apiEquivalentCostMicros / 1_000_000,
    trendPercent: 1,
    trendPreviousTokens: observedTokens - 1_000_000,
    apiEquivalentCostBasis: "Synthetic maximum-size local fixture v1",
    apiEquivalentCostQuality: "modeled",
    apiEquivalentCostCoveragePercent: 100,
  } as const;
}

function usagePeriods(providerIndex: number) {
  const base = 1_000_000_000 + providerIndex * 100_000_000;
  return {
    scanStatus: "complete",
    todayScanStatus: "complete",
    sevenDayScanStatus: "complete",
    thirtyDayScanStatus: "complete",
    today: usageTotal(base, base * 2),
    sevenDays: usageTotal(base * 7, base * 14),
    thirtyDays: usageTotal(base * 30, base * 60),
  } as const;
}

function panelProjection(): PanelProjection {
  const providers = SUPPORTED_PROVIDERS.map((provider, providerIndex) => {
    const usage = usagePeriods(providerIndex);
    return {
      provider,
      displayName: provider === "codex" ? "Codex" : "Claude",
      presence: "detected",
      quota: { availability: "unavailable", provider, quotaLanes: [] },
      usage,
      topModelUsage: {
        model: `synthetic-${provider}-model-01`,
        observedTokens: usage.thirtyDays.observedTokens,
      },
    };
  });
  return {
    contractVersion: 3,
    generatedAt: "2026-08-08T00:00:00Z",
    revision: "1",
    providers,
    combinedUsage: usagePeriods(2),
    sync: { status: "unavailable", lastSuccessfulAt: null },
    profile: { status: "not-authorized" },
  };
}

function createMacosRefreshFixture(): MacosRefreshFixture {
  return {
    version: REFRESH_FIXTURE_VERSION,
    source: "synthetic",
    maxima: {
      global_rows: 100,
      model_cost_days_per_provider: 30,
      my_tokenmaxxers_rows: 100,
      ranking_days_per_provider: 60,
      supported_providers: 2,
    },
    providers: SUPPORTED_PROVIDERS.map(providerFixture),
    doomerboards: {
      global: doomerboardRows("Global"),
      my_tokenmaxxers: doomerboardRows("My Tokenmaxxers"),
    },
    panel_projection: panelProjection(),
  };
}

export function generateMacosRefreshFixtureBytes(): Uint8Array {
  return Buffer.from(`${JSON.stringify(createMacosRefreshFixture(), null, 2)}\n`, "utf8");
}

export function createMacosRefreshFixtureBinding(
  fixtureBytes = generateMacosRefreshFixtureBytes(),
): MacosRefreshFixtureBinding {
  return {
    version: REFRESH_FIXTURE_VERSION,
    sha256: createHash("sha256").update(fixtureBytes).digest("hex"),
    bytes: fixtureBytes.byteLength,
  };
}

function writeFixtureFile(outputFile: string, bytes: Uint8Array) {
  writeFileSync(outputFile, bytes, { flag: "wx", mode: 0o600 });
}

export function runMacosRefreshFixtureCli(
  argumentsList: readonly string[],
  writeFixture: MacosRefreshFixtureWriter = writeFixtureFile,
): MacosRefreshFixtureBinding {
  const outputFile = argumentsList[0];
  if (argumentsList.length !== 1 || !outputFile || outputFile.trim() === "") {
    throw new Error("Usage: bun scripts/macos-refresh-fixture.ts <output-file>");
  }

  const bytes = generateMacosRefreshFixtureBytes();
  writeFixture(outputFile, bytes);
  return createMacosRefreshFixtureBinding(bytes);
}

if (import.meta.main) runMacosRefreshFixtureCli(process.argv.slice(2));
