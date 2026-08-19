import { paginationOptsValidator } from "convex/server";
import { v } from "convex/values";

import { internal } from "../_generated/api";
import { internalMutation } from "../_generated/server";
import { recomputeScores } from "../model/scores";
import { rankingDayAt } from "../model/values";

const RECOMPUTE_PAGE_SIZE = 100;

export const one = internalMutation({
  args: { tokenmaxxerId: v.id("tokenmaxxers") },
  returns: v.null(),
  handler: async (ctx, args) => {
    await recomputeScores(ctx, args.tokenmaxxerId, rankingDayAt());
    return null;
  },
});

export const scheduleAll = internalMutation({
  args: {},
  returns: v.null(),
  handler: async (ctx) => {
    await ctx.scheduler.runAfter(
      0,
      internal.internal.recompute.schedulePage,
      {
        paginationOpts: {
          cursor: null,
          maximumRowsRead: RECOMPUTE_PAGE_SIZE,
          numItems: RECOMPUTE_PAGE_SIZE,
        },
      },
    );
    return null;
  },
});

export const schedulePage = internalMutation({
  args: {
    paginationOpts: paginationOptsValidator,
  },
  returns: v.null(),
  handler: async (ctx, args) => {
    const page = await ctx.db
      .query("tokenmaxxers")
      .withIndex("by_last_synced_at")
      .paginate(args.paginationOpts);
    await Promise.all(
      page.page.map((tokenmaxxer) =>
        ctx.scheduler.runAfter(0, internal.internal.recompute.one, {
          tokenmaxxerId: tokenmaxxer._id,
        }),
      ),
    );
    if (!page.isDone) {
      await ctx.scheduler.runAfter(
        0,
        internal.internal.recompute.schedulePage,
        {
          paginationOpts: {
            cursor: page.continueCursor,
            maximumRowsRead: RECOMPUTE_PAGE_SIZE,
            numItems: RECOMPUTE_PAGE_SIZE,
          },
        },
      );
    }
    return null;
  },
});
