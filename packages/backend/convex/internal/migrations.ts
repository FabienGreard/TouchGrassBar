import { Migrations } from "@convex-dev/migrations";

import { components } from "../_generated/api";
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
      key: doomerboardKey(
        publicUsage.tokenScore,
        publicUsage.touchGrassId,
      ),
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
