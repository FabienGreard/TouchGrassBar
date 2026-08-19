import { v } from "convex/values";

import { internal } from "../_generated/api";
import {
  internalMutation,
  type MutationCtx,
} from "../_generated/server";
import { recomputeScores } from "../model/scores";
import { rankingDayAt } from "../model/values";

const RECOMPUTE_PAGE_SIZE = 5;
const RECOMPUTE_STALL_MS = 15 * 60 * 1_000;
const RECOMPUTE_ALERT_COOLDOWN_MS = 60 * 60 * 1_000;

async function currentDrain(ctx: MutationCtx) {
  const drains = await ctx.db.query("scoreRecomputeDrains").take(2);
  if (drains.length > 1) {
    throw new Error("Score recomputation drain singleton invariant failed");
  }
  return drains[0] ?? null;
}

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
    const now = Date.now();
    const existing = await currentDrain(ctx);
    const generation = (existing?.generation ?? 0) + 1;
    const nextDrain = {
      completedAt: null,
      cursor: null,
      generation,
      lastAlertedAt: null,
      pagesCompleted: 0,
      profilesCompleted: 0,
      startedAt: now,
      status: "running" as const,
      updatedAt: now,
    };
    if (existing) {
      await ctx.db.replace(existing._id, nextDrain);
    } else {
      await ctx.db.insert("scoreRecomputeDrains", nextDrain);
    }
    await ctx.scheduler.runAfter(
      0,
      internal.internal.recompute.schedulePage,
      { cursor: null, generation },
    );
    return null;
  },
});

export const schedulePage = internalMutation({
  args: {
    cursor: v.union(v.string(), v.null()),
    generation: v.number(),
  },
  returns: v.null(),
  handler: async (ctx, args) => {
    const drain = await currentDrain(ctx);
    if (
      !drain ||
      drain.status !== "running" ||
      drain.generation !== args.generation ||
      drain.cursor !== args.cursor
    ) {
      return null;
    }
    const page = await ctx.db
      .query("tokenmaxxers")
      .withIndex("by_last_synced_at")
      .paginate({
        cursor: args.cursor,
        maximumRowsRead: RECOMPUTE_PAGE_SIZE,
        numItems: RECOMPUTE_PAGE_SIZE,
      });
    const rankingDay = rankingDayAt();
    for (const tokenmaxxer of page.page) {
      await recomputeScores(ctx, tokenmaxxer._id, rankingDay);
    }
    const now = Date.now();
    const progress = {
      pagesCompleted: drain.pagesCompleted + 1,
      profilesCompleted: drain.profilesCompleted + page.page.length,
      updatedAt: now,
    };
    if (!page.isDone) {
      await ctx.db.patch(drain._id, {
        ...progress,
        cursor: page.continueCursor,
      });
      await ctx.scheduler.runAfter(
        0,
        internal.internal.recompute.schedulePage,
        { cursor: page.continueCursor, generation: drain.generation },
      );
    } else {
      await ctx.db.patch(drain._id, {
        ...progress,
        completedAt: now,
        cursor: null,
        status: "complete",
      });
    }
    return null;
  },
});

export const monitor = internalMutation({
  args: {},
  returns: v.null(),
  handler: async (ctx) => {
    const drain = await currentDrain(ctx);
    const now = Date.now();
    if (
      !drain ||
      drain.status !== "running" ||
      now - drain.updatedAt < RECOMPUTE_STALL_MS
    ) {
      return null;
    }
    if (
      drain.lastAlertedAt === null ||
      now - drain.lastAlertedAt >= RECOMPUTE_ALERT_COOLDOWN_MS
    ) {
      console.error("Daily score recomputation drain stalled", {
        generation: drain.generation,
        pagesCompleted: drain.pagesCompleted,
        profilesCompleted: drain.profilesCompleted,
      });
      await ctx.db.patch(drain._id, { lastAlertedAt: now });
    }
    await ctx.scheduler.runAfter(
      0,
      internal.internal.recompute.schedulePage,
      { cursor: drain.cursor, generation: drain.generation },
    );
    return null;
  },
});
