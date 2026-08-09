import { Migrations } from "@convex-dev/migrations";

import { components } from "../_generated/api";
import { globalDoomerboardIndex } from "../model/doomerboardIndex";
import schema from "../schema";

export const migrations = new Migrations(components.migrations, { schema });

export const backfillGlobalDoomerboardIndex = migrations.define({
  table: "publicUsages",
  migrateOne: async (ctx, publicUsage) => {
    await globalDoomerboardIndex.insertIfDoesNotExist(ctx, {
      id: publicUsage._id,
      key: publicUsage.tokenScore,
      namespace: publicUsage.boardKey,
    });
  },
});
