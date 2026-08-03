import { v } from "convex/values";

import { mutation } from "./_generated/server";
import { applyUsageSnapshots } from "./model/sync";
import {
  scoreScopeValidator,
  scoreWindowValidator,
  usageSnapshotValidator,
} from "./model/values";

const overviewRow = v.object({
  apiEquivalentCostMicros: v.union(v.number(), v.null()),
  scope: scoreScopeValidator,
  tokenScore: v.number(),
  windowDays: scoreWindowValidator,
});

export const dailyUsage = mutation({
  args: {
    installationId: v.string(),
    snapshots: v.array(usageSnapshotValidator),
  },
  returns: v.object({
    changedBuckets: v.number(),
    overview: v.array(overviewRow),
  }),
  handler: async (ctx, args) => {
    const identity = await ctx.auth.getUserIdentity();
    if (!identity) {
      throw new Error("authentication required");
    }
    return applyUsageSnapshots(ctx, identity.subject, args.installationId, args.snapshots);
  },
});
