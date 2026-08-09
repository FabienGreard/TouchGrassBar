import type { GenericId } from "convex/values";

import type { MutationCtx } from "../_generated/server";
import { doomerboard } from "./doomerboard";
import {
  SCOPES,
  WINDOWS,
  boardKey,
  subtractRankingDays,
  type ApiEquivalentCost,
  type ScoreScope,
  type ScoreWindow,
} from "./values";

type CalculatedScore = {
  apiEquivalentCost: ApiEquivalentCost | null;
  tokenScore: number;
};

type DailyUsageRow = {
  apiEquivalentCost: ApiEquivalentCost | null;
  observedTokens: number;
  provider: string;
  rankingDay: string;
};

function checkedAdd(left: number, right: number, field: string) {
  const sum = left + right;
  if (!Number.isSafeInteger(sum)) {
    throw new Error(`${field} exceeds the safe integer range`);
  }
  return sum;
}

function weakestQuality(
  left: ApiEquivalentCost["quality"],
  right: ApiEquivalentCost["quality"],
): ApiEquivalentCost["quality"] {
  if (left === "modeled" || right === "modeled") return "modeled";
  if (left === "local-only" || right === "local-only") return "local-only";
  return "reconciled";
}

export function calculateScore(
  rows: DailyUsageRow[],
  scope: ScoreScope,
  windowDays: ScoreWindow,
  asOfDay: string,
): CalculatedScore {
  const fromDay = subtractRankingDays(asOfDay, windowDays - 1);
  let tokenScore = 0;
  let costMicros = 0;
  let coveredTokenPercent = 0;
  let quality: ApiEquivalentCost["quality"] = "reconciled";
  let hasPricedEvidence = false;
  let hasUnpricedTokens = false;
  const pricingBases = new Set<string>();

  for (const row of rows) {
    if (row.rankingDay < fromDay || row.rankingDay > asOfDay) {
      continue;
    }
    if (scope !== "combined" && row.provider !== scope) {
      continue;
    }
    tokenScore = checkedAdd(tokenScore, row.observedTokens, "Token Score");
    const cost = row.apiEquivalentCost;
    if (!cost) {
      hasUnpricedTokens ||= row.observedTokens > 0;
      continue;
    }
    hasPricedEvidence = true;
    costMicros = checkedAdd(costMicros, cost.micros, "API-equivalent cost");
    pricingBases.add(cost.pricingBasis);
    quality = weakestQuality(quality, cost.quality);
    const coveragePercent =
      cost.quality === "modeled" ? cost.coveragePercent : 100;
    if (coveragePercent === null) {
      throw new Error("Modeled cost is missing its coverage");
    }
    coveredTokenPercent += coveragePercent * row.observedTokens;
  }

  if (!hasPricedEvidence) {
    return {
      apiEquivalentCost: null,
      tokenScore,
    };
  }
  if (hasUnpricedTokens) quality = "modeled";
  const coveragePercent =
    quality === "modeled"
      ? tokenScore > 0
        ? Math.min(Math.max(coveredTokenPercent / tokenScore, 0), 100)
        : 100
      : null;
  const apiEquivalentCost: ApiEquivalentCost = {
    coveragePercent,
    micros: costMicros,
    pricingBasis: [...pricingBases].sort().join(" + "),
    quality,
  };

  return {
    apiEquivalentCost,
    tokenScore,
  };
}

async function upsertPublicUsage(
  ctx: MutationCtx,
  tokenmaxxer: {
    _id: GenericId<"tokenmaxxers">;
    displayName: string;
    publicId: string;
  },
  scope: ScoreScope,
  windowDays: ScoreWindow,
  score: CalculatedScore,
) {
  const publicUsages = await ctx.db
    .query("publicUsages")
    .withIndex("by_tokenmaxxer_id", (q) =>
      q.eq("tokenmaxxerId", tokenmaxxer._id),
    )
    .take(20);
  const existing = publicUsages.find(
    (row) => row.scope === scope && row.windowDays === windowDays,
  );
  const namespace = boardKey(scope, windowDays);
  const values = {
    boardKey: namespace,
    computedAt: Date.now(),
    displayName: tokenmaxxer.displayName,
    scope,
    tokenScore: score.tokenScore,
    touchGrassId: tokenmaxxer.publicId,
    windowDays,
  };

  if (existing) {
    await ctx.db.patch(existing._id, {
      ...values,
      apiEquivalentCost: score.apiEquivalentCost,
    });
    await doomerboard.replace(
      ctx,
      {
        id: existing._id,
        key: existing.tokenScore,
        namespace: existing.boardKey,
      },
      { key: score.tokenScore, namespace },
    );
    return;
  }

  const publicUsageId = await ctx.db.insert("publicUsages", {
    ...values,
    apiEquivalentCost: score.apiEquivalentCost,
    tokenmaxxerId: tokenmaxxer._id,
  });
  await doomerboard.insert(ctx, {
    id: publicUsageId,
    key: score.tokenScore,
    namespace,
  });
}

export async function recomputeScores(
  ctx: MutationCtx,
  tokenmaxxerId: GenericId<"tokenmaxxers">,
  asOfDay: string,
) {
  const tokenmaxxer = await ctx.db.get(tokenmaxxerId);
  if (!tokenmaxxer) {
    throw new Error("Tokenmaxxer no longer exists");
  }
  const activeDeviceId = tokenmaxxer.activeDeviceId;
  const providerSettings = activeDeviceId
    ? await ctx.db
        .query("deviceProviderSettings")
        .withIndex("by_device_id", (q) => q.eq("deviceId", activeDeviceId))
        .unique()
    : null;
  if (providerSettings && providerSettings.tokenmaxxerId !== tokenmaxxer._id) {
    throw new Error("provider settings owner is invalid");
  }
  const enabledProviders = new Set(
    providerSettings
      ? [
          ...(providerSettings.codexEnabled ? ["codex"] : []),
          ...(providerSettings.claudeEnabled ? ["claude"] : []),
        ]
      : ["codex", "claude"],
  );
  const dailyRows = (
    await ctx.db
      .query("userDailyUsage")
      .withIndex("by_tokenmaxxer_id", (q) =>
        q.eq("tokenmaxxerId", tokenmaxxerId),
      )
      .take(1_000)
  ).filter((row) => enabledProviders.has(row.provider));
  const overview = [];
  for (const scope of SCOPES) {
    for (const windowDays of WINDOWS) {
      const score = calculateScore(dailyRows, scope, windowDays, asOfDay);
      await upsertPublicUsage(ctx, tokenmaxxer, scope, windowDays, score);
      overview.push({
        apiEquivalentCost: score.apiEquivalentCost,
        scope,
        tokenScore: score.tokenScore,
        windowDays,
      });
    }
  }

  return overview;
}
