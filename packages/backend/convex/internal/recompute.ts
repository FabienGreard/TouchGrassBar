import { paginationOptsValidator } from "convex/server";
import { v } from "convex/values";

import { internal } from "../_generated/api";
import { internalMutation } from "../_generated/server";
import { recomputeScores } from "../model/scores";
import { assertRankingDay, rankingDayAt } from "../model/values";

const RECOMPUTE_PAGE_SIZE = 100;

export const one = internalMutation({
  args: {
    rankingDay: v.optional(v.string()),
    tokenmaxxerId: v.id("tokenmaxxers"),
  },
  returns: v.null(),
  handler: async (ctx, args) => {
    const rankingDay = args.rankingDay ?? rankingDayAt();
    assertRankingDay(rankingDay);
    await recomputeScores(ctx, args.tokenmaxxerId, rankingDay);
    return null;
  },
});

export const scheduleRecentlyActive = internalMutation({
  args: {},
  returns: v.null(),
  handler: async (ctx) => {
    const activeSince = Date.now() - 45 * 24 * 60 * 60 * 1_000;
    await ctx.scheduler.runAfter(
      0,
      internal.internal.recompute.scheduleRecentlyActivePage,
      {
        activeSince,
        paginationOpts: {
          cursor: null,
          maximumRowsRead: RECOMPUTE_PAGE_SIZE,
          numItems: RECOMPUTE_PAGE_SIZE,
        },
        rankingDay: rankingDayAt(),
      },
    );
    return null;
  },
});

export const scheduleRecentlyActivePage = internalMutation({
  args: {
    activeSince: v.number(),
    paginationOpts: paginationOptsValidator,
    rankingDay: v.string(),
  },
  returns: v.null(),
  handler: async (ctx, args) => {
    assertRankingDay(args.rankingDay);
    const page = await ctx.db
      .query("tokenmaxxers")
      .withIndex("by_last_synced_at", (q) =>
        q.gte("lastSyncedAt", args.activeSince),
      )
      .paginate(args.paginationOpts);
    await Promise.all(
      page.page.map((tokenmaxxer) =>
        ctx.scheduler.runAfter(0, internal.internal.recompute.one, {
          rankingDay: args.rankingDay,
          tokenmaxxerId: tokenmaxxer._id,
        }),
      ),
    );
    if (!page.isDone) {
      await ctx.scheduler.runAfter(
        0,
        internal.internal.recompute.scheduleRecentlyActivePage,
        {
          activeSince: args.activeSince,
          paginationOpts: {
            cursor: page.continueCursor,
            maximumRowsRead: RECOMPUTE_PAGE_SIZE,
            numItems: RECOMPUTE_PAGE_SIZE,
          },
          rankingDay: args.rankingDay,
        },
      );
    }
    return null;
  },
});
