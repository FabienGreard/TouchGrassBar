import { v } from "convex/values";

import { mutation } from "./_generated/server";
import { requireAuthUser } from "./auth";
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
    const authUser = await requireAuthUser(ctx);
    return applyUsageSnapshots(
      ctx,
      authUser.id,
      args.installationId,
      args.snapshots,
    );
  },
});
