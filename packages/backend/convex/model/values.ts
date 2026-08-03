import { v } from "convex/values";

export const providerValidator = v.union(v.literal("codex"), v.literal("claude"));
export const scoreScopeValidator = v.union(providerValidator, v.literal("combined"));
export const scoreWindowValidator = v.union(v.literal(1), v.literal(7), v.literal(30));
export const coverageValidator = v.union(v.literal("complete"), v.literal("partial"));

export type Provider = "codex" | "claude";
export type ScoreScope = Provider | "combined";
export type ScoreWindow = 1 | 7 | 30;

export const usageSnapshotValidator = v.object({
  apiEquivalentCostMicros: v.union(v.number(), v.null()),
  coverage: coverageValidator,
  observedAt: v.number(),
  observedTokens: v.number(),
  priceBasisVersion: v.union(v.string(), v.null()),
  provider: providerValidator,
  rankingDay: v.string(),
  revision: v.number(),
  source: v.literal("local-observed"),
});

export type UsageSnapshot = {
  apiEquivalentCostMicros: number | null;
  coverage: "complete" | "partial";
  observedAt: number;
  observedTokens: number;
  priceBasisVersion: string | null;
  provider: Provider;
  rankingDay: string;
  revision: number;
  source: "local-observed";
};

export const WINDOWS: readonly ScoreWindow[] = [1, 7, 30];
export const SCOPES: readonly ScoreScope[] = ["codex", "claude", "combined"];

export function boardKey(scope: ScoreScope, windowDays: ScoreWindow) {
  return `tokens-v1:${scope}:${windowDays}d`;
}

export function assertUsageSnapshot(snapshot: UsageSnapshot) {
  if (!/^\d{4}-\d{2}-\d{2}$/.test(snapshot.rankingDay)) {
    throw new Error("rankingDay must be a UTC date in YYYY-MM-DD form");
  }
  const canonicalDay = new Date(`${snapshot.rankingDay}T00:00:00.000Z`)
    .toISOString()
    .slice(0, 10);
  if (canonicalDay !== snapshot.rankingDay) {
    throw new Error("rankingDay is not a real UTC calendar day");
  }
  if (!Number.isSafeInteger(snapshot.revision) || snapshot.revision < 1) {
    throw new Error("revision must be a positive safe integer");
  }
  if (!Number.isSafeInteger(snapshot.observedTokens) || snapshot.observedTokens < 0) {
    throw new Error("observedTokens must be a non-negative safe integer");
  }
  if (
    snapshot.apiEquivalentCostMicros !== null &&
    (!Number.isSafeInteger(snapshot.apiEquivalentCostMicros) ||
      snapshot.apiEquivalentCostMicros < 0)
  ) {
    throw new Error("apiEquivalentCostMicros must be null or a non-negative safe integer");
  }
}

export function rankingDayAt(timestamp = Date.now()) {
  return new Date(timestamp).toISOString().slice(0, 10);
}

export function subtractRankingDays(rankingDay: string, days: number) {
  const date = new Date(`${rankingDay}T00:00:00.000Z`);
  date.setUTCDate(date.getUTCDate() - days);
  return date.toISOString().slice(0, 10);
}
