import type { GenericId } from "convex/values";

import type { MutationCtx } from "../_generated/server";
import { globalDoomerboard } from "./aggregate";
import {
  SCOPES,
  WINDOWS,
  boardKey,
  subtractRankingDays,
  type ScoreScope,
  type ScoreWindow,
} from "./values";

type CalculatedScore = {
  apiEquivalentCostMicros: number | undefined;
  tokenScore: number;
};

export function calculateScore(
  rows: Array<{
    apiEquivalentCostMicros?: number;
    costIsComplete: boolean;
    observedTokens: number;
    provider: string;
    rankingDay: string;
  }>,
  scope: ScoreScope,
  windowDays: ScoreWindow,
  asOfDay: string,
): CalculatedScore {
  const fromDay = subtractRankingDays(asOfDay, windowDays - 1);
  let tokenScore = 0;
  let apiEquivalentCostMicros = 0;
  let costIsComplete = true;

  for (const row of rows) {
    if (row.rankingDay < fromDay || row.rankingDay > asOfDay) {
      continue;
    }
    if (scope !== "combined" && row.provider !== scope) {
      continue;
    }
    tokenScore += row.observedTokens;
    apiEquivalentCostMicros += row.apiEquivalentCostMicros ?? 0;
    if (!row.costIsComplete && row.observedTokens > 0) {
      costIsComplete = false;
    }
  }

  return {
    apiEquivalentCostMicros: costIsComplete ? apiEquivalentCostMicros : undefined,
    tokenScore,
  };
}

async function upsertPublicScore(
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
  const publicRows = await ctx.db
    .query("publicScores")
    .withIndex("by_tokenmaxxer_id", (q) =>
      q.eq("tokenmaxxerId", tokenmaxxer._id),
    )
    .take(20);
  const existing = publicRows.find(
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
      apiEquivalentCostMicros: score.apiEquivalentCostMicros,
    });
    await globalDoomerboard.replace(
      ctx,
      { id: existing._id, key: existing.tokenScore, namespace: existing.boardKey },
      { key: score.tokenScore, namespace },
    );
    return;
  }

  const publicScoreId = await ctx.db.insert("publicScores", {
    ...values,
    ...(score.apiEquivalentCostMicros === undefined
      ? {}
      : { apiEquivalentCostMicros: score.apiEquivalentCostMicros }),
    tokenmaxxerId: tokenmaxxer._id,
  });
  await globalDoomerboard.insert(ctx, {
    id: publicScoreId,
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
  const dailyRows = await ctx.db
    .query("userDailyUsage")
    .withIndex("by_tokenmaxxer_id", (q) => q.eq("tokenmaxxerId", tokenmaxxerId))
    .take(1_000);
  const existingScores = await ctx.db
    .query("userScores")
    .withIndex("by_tokenmaxxer_id", (q) => q.eq("tokenmaxxerId", tokenmaxxerId))
    .take(20);

  const overview = [];
  for (const scope of SCOPES) {
    for (const windowDays of WINDOWS) {
      const score = calculateScore(dailyRows, scope, windowDays, asOfDay);
      const existing = existingScores.find(
        (candidate) => candidate.scope === scope && candidate.windowDays === windowDays,
      );
      const values = {
        boardKey: boardKey(scope, windowDays),
        computedAt: Date.now(),
        scope,
        tokenScore: score.tokenScore,
        windowDays,
      };
      if (existing) {
        await ctx.db.patch(existing._id, {
          ...values,
          apiEquivalentCostMicros: score.apiEquivalentCostMicros,
        });
      } else {
        await ctx.db.insert("userScores", {
          ...values,
          ...(score.apiEquivalentCostMicros === undefined
            ? {}
            : { apiEquivalentCostMicros: score.apiEquivalentCostMicros }),
          tokenmaxxerId,
        });
      }
      await upsertPublicScore(ctx, tokenmaxxer, scope, windowDays, score);
      overview.push({
        apiEquivalentCostMicros: score.apiEquivalentCostMicros ?? null,
        scope,
        tokenScore: score.tokenScore,
        windowDays,
      });
    }
  }

  return overview;
}
