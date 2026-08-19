import { v } from "convex/values";

import { mutation } from "./_generated/server";
import { requireAuthUser } from "./auth";
import { applyProviderSettings, applyUsageSnapshots } from "./model/sync";
import { providerValidator, usageSnapshotValidator } from "./model/values";

const acknowledgement = v.object({
  outcome: v.union(
    v.literal("committed"),
    v.literal("conflict"),
    v.literal("idempotent"),
    v.literal("stale"),
  ),
  provider: providerValidator,
  rankingDay: v.string(),
  revision: v.number(),
});

const providerSettingsAcknowledgement = v.object({
  outcome: v.union(v.literal("committed"), v.literal("idempotent"), v.literal("stale")),
  revision: v.number(),
});

export const providerSettings = mutation({
  args: {
    activeMacGeneration: v.number(),
    enabledProviders: v.array(providerValidator),
    installationCredential: v.string(),
    revision: v.number(),
  },
  returns: providerSettingsAcknowledgement,
  handler: async (ctx, args) => {
    const authUser = await requireAuthUser(ctx);
    return applyProviderSettings(
      ctx,
      authUser,
      args.installationCredential,
      args.activeMacGeneration,
      args.revision,
      args.enabledProviders,
    );
  },
});

export const dailyUsage = mutation({
  args: {
    activeMacGeneration: v.number(),
    installationCredential: v.string(),
    profileBackfillAnchor: v.union(v.string(), v.null()),
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
      args.profileBackfillAnchor,
    );
  },
});
