import { v } from "convex/values";

import { internalQuery } from "../_generated/server";

const PAGE_SIZE = 100;

const resultValidator = v.object({
  continueCursor: v.string(),
  invalidActiveMacAuthorities: v.number(),
  isDone: v.boolean(),
  missingActiveAuthSessionIds: v.number(),
  missingAuthSessionGenerations: v.number(),
  processedProfiles: v.number(),
  profilesMissingFenceFields: v.number(),
});

export const check = internalQuery({
  args: { cursor: v.union(v.string(), v.null()) },
  returns: resultValidator,
  handler: async (ctx, args) => {
    const page = await ctx.db.query("tokenmaxxers").paginate({
      cursor: args.cursor,
      maximumRowsRead: PAGE_SIZE,
      numItems: PAGE_SIZE,
    });
    let invalidActiveMacAuthorities = 0;
    let missingActiveAuthSessionIds = 0;
    let missingAuthSessionGenerations = 0;
    let profilesMissingFenceFields = 0;
    for (const profile of page.page) {
      const activeAuthSessionIdMissing = profile.activeAuthSessionId === undefined;
      const authSessionGenerationMissing = profile.authSessionGeneration === undefined;
      const activeDevice = profile.activeDeviceId ? await ctx.db.get(profile.activeDeviceId) : null;
      if (
        !activeDevice ||
        activeDevice.tokenmaxxerId !== profile._id ||
        activeDevice.revokedAt !== undefined ||
        !Number.isSafeInteger(activeDevice.generation) ||
        activeDevice.generation < 1
      ) {
        invalidActiveMacAuthorities += 1;
      }
      if (activeAuthSessionIdMissing) missingActiveAuthSessionIds += 1;
      if (authSessionGenerationMissing) missingAuthSessionGenerations += 1;
      if (activeAuthSessionIdMissing || authSessionGenerationMissing) {
        profilesMissingFenceFields += 1;
      }
    }
    return {
      continueCursor: page.continueCursor,
      invalidActiveMacAuthorities,
      isDone: page.isDone,
      missingActiveAuthSessionIds,
      missingAuthSessionGenerations,
      processedProfiles: page.page.length,
      profilesMissingFenceFields,
    };
  },
});
