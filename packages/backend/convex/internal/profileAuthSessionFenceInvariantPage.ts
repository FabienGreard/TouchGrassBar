import { paginationOptsValidator, paginationResultValidator } from "convex/server";
import { v } from "convex/values";

import { internalQuery } from "../_generated/server";

const profileFenceStateValidator = v.object({
  activeAuthSessionIdMissing: v.boolean(),
  activeMacAuthorityInvalid: v.boolean(),
  authSessionGenerationMissing: v.boolean(),
});

export const states = internalQuery({
  args: { paginationOpts: paginationOptsValidator },
  returns: paginationResultValidator(profileFenceStateValidator),
  handler: async (ctx, args) => {
    const result = await ctx.db.query("tokenmaxxers").paginate(args.paginationOpts);
    const page = [];
    for (const profile of result.page) {
      const activeDevice = profile.activeDeviceId ? await ctx.db.get(profile.activeDeviceId) : null;
      page.push({
        activeAuthSessionIdMissing: profile.activeAuthSessionId === undefined,
        activeMacAuthorityInvalid:
          !activeDevice ||
          activeDevice.tokenmaxxerId !== profile._id ||
          activeDevice.revokedAt !== undefined ||
          !Number.isSafeInteger(activeDevice.generation) ||
          activeDevice.generation < 1,
        authSessionGenerationMissing: profile.authSessionGeneration === undefined,
      });
    }
    return {
      ...result,
      page,
    };
  },
});
