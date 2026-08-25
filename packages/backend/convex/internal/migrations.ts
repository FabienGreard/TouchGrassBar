import { Migrations } from "@convex-dev/migrations";

import { components } from "../_generated/api";
import schema from "../schema";

export const migrations = new Migrations(components.migrations, { schema });

export const backfillDeviceUsageCompletion = migrations.define({
  table: "devices",
  migrateOne: async (ctx, device) => {
    if (device.usageBackfillCompletedAt === undefined) {
      await ctx.db.patch(device._id, { usageBackfillCompletedAt: null });
    }
  },
});
