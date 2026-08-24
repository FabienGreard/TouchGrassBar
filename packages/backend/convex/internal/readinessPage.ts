import { paginationOptsValidator, paginationResultValidator } from "convex/server";
import { v } from "convex/values";

import { internalQuery } from "../_generated/server";

const deviceMigrationRow = v.object({
  hasCompletionField: v.boolean(),
});

export const canaries = internalQuery({
  args: { paginationOpts: paginationOptsValidator },
  returns: paginationResultValidator(v.null()),
  handler: async (ctx, args) => {
    const result = await ctx.db.query("readinessCanaries").paginate(args.paginationOpts);
    return { ...result, page: result.page.map(() => null) };
  },
});

export const devices = internalQuery({
  args: { paginationOpts: paginationOptsValidator },
  returns: paginationResultValidator(deviceMigrationRow),
  handler: async (ctx, args) => {
    const result = await ctx.db.query("devices").paginate(args.paginationOpts);
    return {
      ...result,
      page: result.page.map((device) => ({
        hasCompletionField: device.usageBackfillCompletedAt !== undefined,
      })),
    };
  },
});
