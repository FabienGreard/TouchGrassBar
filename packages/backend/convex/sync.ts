import { v } from "convex/values";

import { mutation } from "./_generated/server";
import { requireAuthUser } from "./auth";
import { applyUsageSnapshots } from "./model/sync";
import {
  providerValidator,
  usageSnapshotValidator,
} from "./model/values";

const acknowledgement = v.object({
  outcome: v.union(
    v.literal("committed"),
    v.literal("idempotent"),
    v.literal("stale"),
  ),
  provider: providerValidator,
  rankingDay: v.string(),
  revision: v.number(),
});

export const dailyUsage = mutation({
  args: {
    activeMacGeneration: v.number(),
    installationCredential: v.string(),
    snapshots: v.array(usageSnapshotValidator),
  },
  returns: v.array(acknowledgement),
  handler: async (ctx, args) => {
    const authUser = await requireAuthUser(ctx);
    return applyUsageSnapshots(
      ctx,
      authUser,
      args.installationCredential,
      args.activeMacGeneration,
      args.snapshots,
    );
  },
});
