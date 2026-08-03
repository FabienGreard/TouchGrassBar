import { Migrations } from "@convex-dev/migrations";

import { components } from "../_generated/api";
import { globalDoomerboard } from "../model/aggregate";
import schema from "../schema";

export const migrations = new Migrations(components.migrations, { schema });

export const backfillPublicScoreAggregate = migrations.define({
  table: "publicScores",
  migrateOne: async (ctx, publicScore) => {
    await globalDoomerboard.insertIfDoesNotExist(ctx, {
      id: publicScore._id,
      key: publicScore.tokenScore,
      namespace: publicScore.boardKey,
    });
  },
});
