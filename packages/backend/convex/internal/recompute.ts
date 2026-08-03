import { v } from "convex/values";

import { internal } from "../_generated/api";
import { internalMutation } from "../_generated/server";
import { recomputeScores } from "../model/scores";
import { rankingDayAt } from "../model/values";

export const one = internalMutation({
  args: { tokenmaxxerId: v.id("tokenmaxxers") },
  returns: v.null(),
  handler: async (ctx, args) => {
    await recomputeScores(ctx, args.tokenmaxxerId, rankingDayAt());
    return null;
  },
});

export const scheduleRecentlyActive = internalMutation({
  args: {},
  returns: v.null(),
  handler: async (ctx) => {
    const activeSince = Date.now() - 45 * 24 * 60 * 60 * 1_000;
    const tokenmaxxers = await ctx.db
      .query("tokenmaxxers")
      .withIndex("by_last_synced_at", (q) => q.gte("lastSyncedAt", activeSince))
      .take(200);
    await Promise.all(
      tokenmaxxers.map((tokenmaxxer) =>
        ctx.scheduler.runAfter(0, internal.internal.recompute.one, {
          tokenmaxxerId: tokenmaxxer._id,
        }),
      ),
    );
    return null;
  },
});
