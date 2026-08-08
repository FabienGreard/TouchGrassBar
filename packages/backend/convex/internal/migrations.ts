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

export const retireLegacyActiveDeviceAuthority = migrations.define({
  table: "tokenmaxxers",
  migrateOne: async (ctx, tokenmaxxer) => {
    if (!tokenmaxxer.activeDeviceId) return;
    const device = await ctx.db.get(tokenmaxxer.activeDeviceId);
    if (
      !device ||
      device.tokenmaxxerId !== tokenmaxxer._id ||
      device.installationId === undefined ||
      device.installationCredentialDigest !== undefined ||
      device.generation !== undefined
    ) {
      return;
    }
    await ctx.db.patch(device._id, {
      installationId: undefined,
      revokedAt: Date.now(),
    });
    await ctx.db.patch(tokenmaxxer._id, { activeDeviceId: undefined });
  },
});
