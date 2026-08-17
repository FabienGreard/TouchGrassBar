import { v } from "convex/values";

import type { Doc } from "./_generated/dataModel";
import { query, type QueryCtx } from "./_generated/server";
import { requireAuthUser } from "./auth";
import { doomerboard } from "./model/doomerboard";
import { rejectAuthority } from "./model/authority";
import { tokenmaxxerForAuthUser } from "./model/profile";
import {
  apiEquivalentCostValidator,
  assertRankingDay,
  boardKey,
  rankingDayAt,
  scoreScopeValidator,
  scoreWindowValidator,
  type ApiEquivalentCost,
} from "./model/values";

const doomerboardRow = v.object({
  apiEquivalentCost: apiEquivalentCostValidator,
  displayName: v.string(),
  rank: v.number(),
  tokenScore: v.number(),
  touchGrassId: v.string(),
});

export function rankRows<
  T extends {
    apiEquivalentCost: ApiEquivalentCost | null;
    displayName: string;
    tokenScore: number;
    touchGrassId: string;
  },
>(rows: T[]) {
  const orderedRows = [...rows].sort(
    (left, right) =>
      right.tokenScore - left.tokenScore ||
      left.touchGrassId.localeCompare(right.touchGrassId),
  );
  let rank = 0;
  let previousScore: number | null = null;
  return orderedRows.map((row, index) => {
    if (row.tokenScore !== previousScore) {
      rank = index + 1;
      previousScore = row.tokenScore;
    }
    return {
      apiEquivalentCost: row.apiEquivalentCost,
      displayName: row.displayName,
      rank,
      tokenScore: row.tokenScore,
      touchGrassId: row.touchGrassId,
    };
  });
}

async function requireDoomerboardProfile(ctx: QueryCtx) {
  const authUser = await requireAuthUser(ctx);
  const tokenmaxxer = await tokenmaxxerForAuthUser(ctx, authUser);
  if (!tokenmaxxer) return rejectAuthority();
  return tokenmaxxer;
}

async function globalRows(
  ctx: QueryCtx,
  scope: Parameters<typeof boardKey>[0],
  windowDays: Parameters<typeof boardKey>[1],
  requestedLimit?: number,
  requiredComputedRankingDay?: string,
) {
  await requireDoomerboardProfile(ctx);
  const limit = Math.min(Math.max(Math.floor(requestedLimit ?? 50), 1), 100);
  const rows: Doc<"publicUsages">[] = [];
  let cursor: string | undefined;
  let isDone = false;
  while (rows.length < limit && !isDone) {
    const page = await doomerboard.paginate(ctx, {
      ...(cursor === undefined ? {} : { cursor }),
      namespace: boardKey(scope, windowDays),
      order: "asc",
      pageSize: limit,
    });
    const candidates = await Promise.all(
      page.page.map((item) => ctx.db.get(item.id)),
    );
    rows.push(
      ...candidates.filter(
        (row): row is Exclude<typeof row, null> =>
          row !== null &&
          (requiredComputedRankingDay === undefined ||
            rankingDayAt(row.computedAt) === requiredComputedRankingDay),
      ),
    );
    cursor = page.cursor;
    isDone = page.isDone;
  }
  return rankRows(rows).slice(0, limit);
}

export const currentGlobal = query({
  args: {
    limit: v.optional(v.number()),
    rankingDay: v.string(),
  },
  returns: v.array(doomerboardRow),
  handler: (ctx, args) => {
    assertRankingDay(args.rankingDay);
    return globalRows(ctx, "combined", 1, args.limit, args.rankingDay);
  },
});

export const global = query({
  args: {
    limit: v.optional(v.number()),
    scope: scoreScopeValidator,
    windowDays: scoreWindowValidator,
  },
  returns: v.array(doomerboardRow),
  handler: (ctx, args) =>
    globalRows(ctx, args.scope, args.windowDays, args.limit),
});

export const myTokenmaxxers = query({
  args: {
    scope: scoreScopeValidator,
    windowDays: scoreWindowValidator,
  },
  returns: v.array(doomerboardRow),
  handler: async (ctx, args) => {
    const authUser = await requireAuthUser(ctx);
    const owner = await tokenmaxxerForAuthUser(ctx, authUser);
    if (!owner) {
      return [];
    }
    const added = await ctx.db
      .query("addedTokenmaxxers")
      .withIndex("by_owner_id", (q) => q.eq("ownerId", owner._id))
      .take(500);
    const includedIds = new Set(added.map((edge) => edge.addedId));
    const candidates = await ctx.db
      .query("publicUsages")
      .withIndex("by_board_key", (q) =>
        q.eq("boardKey", boardKey(args.scope, args.windowDays)),
      )
      .take(2_000);
    const rows = candidates.filter((score) =>
      includedIds.has(score.tokenmaxxerId),
    );
    return rankRows(rows);
  },
});
