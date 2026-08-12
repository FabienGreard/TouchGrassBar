import { type Infer, v } from "convex/values";

export const providerValidator = v.union(v.literal("codex"), v.literal("claude"));
export const scoreScopeValidator = v.union(providerValidator, v.literal("combined"));
export const scoreWindowValidator = v.union(v.literal(1), v.literal(7), v.literal(30));
export const coverageValidator = v.union(v.literal("complete"), v.literal("partial"));
export const evidenceBasisValidator = v.union(
  v.literal("provider-reported"),
  v.literal("locally-derived"),
);
export const costQualityValidator = v.union(
  v.literal("reconciled"),
  v.literal("modeled"),
  v.literal("local-only"),
);
export const correctionReasonValidator = v.union(
  v.literal("provider-replacement"),
  v.literal("parser-correction"),
);
export const apiEquivalentCostValueValidator = v.object({
  coveragePercent: v.union(v.number(), v.null()),
  micros: v.number(),
  pricingBasis: v.string(),
  quality: costQualityValidator,
});
export const apiEquivalentCostValidator = v.union(
  v.null(),
  apiEquivalentCostValueValidator,
);

const MAX_OBSERVED_AT_FUTURE_SKEW_MS = 5 * 60 * 1_000;

export const usageSnapshotValidator = v.object({
  apiEquivalentCost: apiEquivalentCostValidator,
  correctionReason: v.union(correctionReasonValidator, v.null()),
  correctionRevision: v.union(v.number(), v.null()),
  coverage: coverageValidator,
  evidenceBasis: evidenceBasisValidator,
  observedAt: v.number(),
  observedTokens: v.number(),
  provider: providerValidator,
  rankingDay: v.string(),
  revision: v.number(),
});

export type Provider = Infer<typeof providerValidator>;
export type ScoreScope = Infer<typeof scoreScopeValidator>;
export type ScoreWindow = Infer<typeof scoreWindowValidator>;
export type ApiEquivalentCost = Infer<typeof apiEquivalentCostValueValidator>;
export type UsageSnapshot = Infer<typeof usageSnapshotValidator>;

// Keep a bounded catalog of bases that retained Daily Usage can still cite.
// Remove an old basis only after no retained provider-day can reference it.
const APPROVED_PRICING_BASES_BY_PROVIDER: Record<
  Provider,
  readonly string[]
> = {
  claude: ["anthropic-standard-2026-08-07-v1"],
  codex: [
    "openai-api-2026-08-09-v3",
    "openai-standard-2026-08-06-v1",
  ],
};

export const WINDOWS: readonly ScoreWindow[] = [1, 7, 30];
export const SCOPES: readonly ScoreScope[] = ["codex", "claude", "combined"];

export function boardKey(scope: ScoreScope, windowDays: ScoreWindow) {
  return `tokens-v1:${scope}:${windowDays}d`;
}

export function assertUsageSnapshot(
  snapshot: UsageSnapshot,
  currentRankingDay = rankingDayAt(),
  now = Date.now(),
  observationMayFollowRankingDay = false,
) {
  if (!/^\d{4}-\d{2}-\d{2}$/.test(snapshot.rankingDay)) {
    throw new Error("rankingDay must be a UTC date in YYYY-MM-DD form");
  }
  const canonicalDay = new Date(`${snapshot.rankingDay}T00:00:00.000Z`)
    .toISOString()
    .slice(0, 10);
  if (canonicalDay !== snapshot.rankingDay) {
    throw new Error("rankingDay is not a real UTC calendar day");
  }
  if (snapshot.rankingDay !== currentRankingDay) {
    throw new Error("rankingDay must be the current UTC Ranking Day");
  }
  if (!Number.isSafeInteger(snapshot.revision) || snapshot.revision < 1) {
    throw new Error("revision must be a positive safe integer");
  }
  if (
    (snapshot.correctionReason === null) !==
    (snapshot.correctionRevision === null)
  ) {
    throw new Error(
      "correctionReason and correctionRevision must both be null or both be set",
    );
  }
  if (
    snapshot.correctionRevision !== null &&
    (!Number.isSafeInteger(snapshot.correctionRevision) ||
      snapshot.correctionRevision < 1 ||
      snapshot.correctionRevision > snapshot.revision)
  ) {
    throw new Error(
      "correctionRevision must be a positive safe integer at or before revision",
    );
  }
  if (!Number.isSafeInteger(snapshot.observedTokens) || snapshot.observedTokens < 0) {
    throw new Error("observedTokens must be a non-negative safe integer");
  }
  if (!Number.isSafeInteger(snapshot.observedAt) || snapshot.observedAt < 0) {
    throw new Error("observedAt must be a non-negative safe integer");
  }
  const rankingDayStart = Date.parse(`${currentRankingDay}T00:00:00.000Z`);
  const rankingDayEnd = rankingDayStart + 24 * 60 * 60 * 1_000;
  if (
    snapshot.observedAt < rankingDayStart ||
    (!observationMayFollowRankingDay && snapshot.observedAt >= rankingDayEnd)
  ) {
    throw new Error("observedAt is outside the approved UTC Ranking Day policy");
  }
  if (snapshot.observedAt > now + MAX_OBSERVED_AT_FUTURE_SKEW_MS) {
    throw new Error("observedAt exceeds the allowed clock-skew window");
  }
  const cost = snapshot.apiEquivalentCost;
  if (cost === null) return;
  if (!Number.isSafeInteger(cost.micros) || cost.micros < 0) {
    throw new Error("cost micros must be a non-negative safe integer");
  }
  if (
    cost.pricingBasis.length < 1 ||
    cost.pricingBasis.length > 256 ||
    !/^[A-Za-z0-9][A-Za-z0-9 ._:+-]*$/.test(cost.pricingBasis)
  ) {
    throw new Error("pricingBasis must be a bounded catalog identifier");
  }
  if (
    !APPROVED_PRICING_BASES_BY_PROVIDER[snapshot.provider].includes(
      cost.pricingBasis,
    )
  ) {
    throw new Error("pricingBasis is not approved for this provider");
  }
  if (
    (snapshot.evidenceBasis === "provider-reported" &&
      cost.quality === "local-only") ||
    (snapshot.evidenceBasis === "locally-derived" &&
      cost.quality === "reconciled")
  ) {
    throw new Error("cost quality does not match the evidence basis");
  }
  if (cost.quality === "modeled") {
    if (
      cost.coveragePercent === null ||
      !Number.isFinite(cost.coveragePercent) ||
      cost.coveragePercent < 0 ||
      cost.coveragePercent > 100
    ) {
      throw new Error("modeled cost requires coveragePercent between 0 and 100");
    }
  } else if (cost.coveragePercent !== null) {
    throw new Error("fixed-quality cost must not include coveragePercent");
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
