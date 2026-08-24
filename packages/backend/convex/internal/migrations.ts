import { Migrations } from "@convex-dev/migrations";
import { v } from "convex/values";

import { components } from "../_generated/api";
import { internalMutation } from "../_generated/server";
import { doomerboard, doomerboardKey } from "../model/doomerboard";
import { markDoomerboardChanged } from "../model/doomerboardVersion";
import schema from "../schema";

export const migrations = new Migrations(components.migrations, { schema });

export const backfillDoomerboard = migrations.define({
  table: "publicUsages",
  migrateOne: async (ctx, publicUsage) => {
    await doomerboard.deleteIfExists(ctx, {
      id: publicUsage._id,
      key: publicUsage.tokenScore,
      namespace: publicUsage.boardKey,
    });
    await doomerboard.insertIfDoesNotExist(ctx, {
      id: publicUsage._id,
      key: doomerboardKey(publicUsage.tokenScore, publicUsage.touchGrassId),
      namespace: publicUsage.boardKey,
    });
    await markDoomerboardChanged(ctx);
  },
});

export const backfillDeviceUsageCompletion = migrations.define({
  table: "devices",
  migrateOne: async (ctx, device) => {
    if (device.usageBackfillCompletedAt === undefined) {
      await ctx.db.patch(device._id, { usageBackfillCompletedAt: null });
    }
  },
});

const PROFILE_AUTH_SESSION_FENCE_BATCH_SIZE = 25;

export const backfillProfileAuthSessionFence = internalMutation({
  args: { cursor: v.union(v.string(), v.null()) },
  returns: v.object({
    changedProfiles: v.number(),
    continueCursor: v.string(),
    invalidActiveMacAuthorities: v.number(),
    isDone: v.boolean(),
    processedProfiles: v.number(),
  }),
  handler: async (ctx, args) => {
    const page = await ctx.db.query("tokenmaxxers").paginate({
      cursor: args.cursor,
      maximumRowsRead: PROFILE_AUTH_SESSION_FENCE_BATCH_SIZE,
      numItems: PROFILE_AUTH_SESSION_FENCE_BATCH_SIZE,
    });
    let changedProfiles = 0;
    let invalidActiveMacAuthorities = 0;
    for (const profile of page.page) {
      if (
        profile.activeAuthSessionId !== undefined &&
        profile.authSessionGeneration !== undefined
      ) {
        continue;
      }
      const activeDevice = profile.activeDeviceId ? await ctx.db.get(profile.activeDeviceId) : null;
      if (
        !activeDevice ||
        activeDevice.tokenmaxxerId !== profile._id ||
        activeDevice.revokedAt !== undefined ||
        !Number.isSafeInteger(activeDevice.generation) ||
        activeDevice.generation < 1
      ) {
        invalidActiveMacAuthorities += 1;
        continue;
      }
      await ctx.db.patch(profile._id, {
        ...(profile.activeAuthSessionId === undefined ? { activeAuthSessionId: null } : {}),
        ...(profile.authSessionGeneration === undefined
          ? { authSessionGeneration: activeDevice.generation }
          : {}),
      });
      changedProfiles += 1;
    }
    return {
      changedProfiles,
      continueCursor: page.continueCursor,
      invalidActiveMacAuthorities,
      isDone: page.isDone,
      processedProfiles: page.page.length,
    };
  },
});
