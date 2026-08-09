import { v } from "convex/values";

import { query } from "./_generated/server";
import { requireAuthUser } from "./auth";
import { globalDoomerboardIndex } from "./model/doomerboardIndex";
import { tokenmaxxerForAuthUser } from "./model/profile";
import {
  apiEquivalentCostValidator,
  boardKey,
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
  let rank = 0;
  let previousScore: number | null = null;
  return rows.map((row, index) => {
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

export const global = query({
  args: {
    limit: v.optional(v.number()),
    scope: scoreScopeValidator,
    windowDays: scoreWindowValidator,
  },
  returns: v.array(doomerboardRow),
  handler: async (ctx, args) => {
    const limit = Math.min(Math.max(Math.floor(args.limit ?? 50), 1), 100);
    const { page } = await globalDoomerboardIndex.paginate(ctx, {
      namespace: boardKey(args.scope, args.windowDays),
      order: "desc",
      pageSize: limit,
    });
    const rows = await Promise.all(page.map((item) => ctx.db.get(item.id)));
    return rankRows(rows.filter((row) => row !== null));
  },
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
      .query("publicScores")
      .withIndex("by_board_key", (q) =>
        q.eq("boardKey", boardKey(args.scope, args.windowDays)),
      )
      .take(2_000);
    const rows = candidates
      .filter((score) => includedIds.has(score.tokenmaxxerId))
      .sort(
        (left, right) =>
          right.tokenScore - left.tokenScore ||
          left.touchGrassId.localeCompare(right.touchGrassId),
      );
    return rankRows(rows);
  },
});
