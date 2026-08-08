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

export const upgradePrecontractUsageBuckets = migrations.define({
  table: "usageBuckets",
  migrateOne: async (ctx, bucket) => {
    const hasCurrentShape =
      bucket.apiEquivalentCost !== undefined &&
      bucket.correctionReason !== undefined &&
      bucket.correctionRevision !== undefined &&
      bucket.evidenceBasis !== undefined;
    if (
      hasCurrentShape &&
      bucket.apiEquivalentCostMicros === undefined &&
      bucket.priceBasisVersion === undefined &&
      bucket.source === undefined
    ) {
      return;
    }
    const correction =
      bucket.correctionReason !== undefined &&
      bucket.correctionRevision !== undefined &&
      ((bucket.correctionReason === null && bucket.correctionRevision === null) ||
        (bucket.correctionReason !== null && bucket.correctionRevision !== null))
        ? {
            reason: bucket.correctionReason,
            revision: bucket.correctionRevision,
          }
        : { reason: null, revision: null };
    await ctx.db.replace(bucket._id, {
      apiEquivalentCost: bucket.apiEquivalentCost ?? null,
      correctionReason: correction.reason,
      correctionRevision: correction.revision,
      coverage: bucket.coverage,
      deviceId: bucket.deviceId,
      evidenceBasis: bucket.evidenceBasis ?? "locally-derived",
      ...(hasCurrentShape &&
      bucket.lastCorrectionReason !== undefined &&
      bucket.lastCorrectionRevision !== undefined
        ? {
            lastCorrectionReason: bucket.lastCorrectionReason,
            lastCorrectionRevision: bucket.lastCorrectionRevision,
          }
        : {}),
      observedAt: bucket.observedAt,
      observedTokens: bucket.observedTokens,
      provider: bucket.provider,
      rankingDay: bucket.rankingDay,
      revision: bucket.revision,
      syncedAt: bucket.syncedAt,
      tokenmaxxerId: bucket.tokenmaxxerId,
    });
  },
});

export const upgradePrecontractUserDailyUsage = migrations.define({
  table: "userDailyUsage",
  migrateOne: async (ctx, usage) => {
    if (
      usage.apiEquivalentCost !== undefined &&
      usage.apiEquivalentCostMicros === undefined &&
      usage.costIsComplete === undefined
    ) {
      return;
    }
    await ctx.db.replace(usage._id, {
      apiEquivalentCost: usage.apiEquivalentCost ?? null,
      observedTokens: usage.observedTokens,
      provider: usage.provider,
      rankingDay: usage.rankingDay,
      tokenmaxxerId: usage.tokenmaxxerId,
      updatedAt: usage.updatedAt,
    });
  },
});

export const upgradePrecontractUserScores = migrations.define({
  table: "userScores",
  migrateOne: async (ctx, score) => {
    if (
      score.apiEquivalentCost !== undefined &&
      score.apiEquivalentCostMicros === undefined
    ) {
      return;
    }
    await ctx.db.replace(score._id, {
      apiEquivalentCost: score.apiEquivalentCost ?? null,
      boardKey: score.boardKey,
      computedAt: score.computedAt,
      scope: score.scope,
      tokenmaxxerId: score.tokenmaxxerId,
      tokenScore: score.tokenScore,
      windowDays: score.windowDays,
    });
  },
});

export const upgradePrecontractPublicScores = migrations.define({
  table: "publicScores",
  migrateOne: async (ctx, score) => {
    if (
      score.apiEquivalentCost !== undefined &&
      score.apiEquivalentCostMicros === undefined
    ) {
      return;
    }
    const aggregateValue = {
      id: score._id,
      key: score.tokenScore,
      namespace: score.boardKey,
    };
    await globalDoomerboard.insertIfDoesNotExist(ctx, aggregateValue);
    await ctx.db.replace(score._id, {
      apiEquivalentCost: score.apiEquivalentCost ?? null,
      boardKey: score.boardKey,
      computedAt: score.computedAt,
      displayName: score.displayName,
      scope: score.scope,
      tokenmaxxerId: score.tokenmaxxerId,
      tokenScore: score.tokenScore,
      touchGrassId: score.touchGrassId,
      windowDays: score.windowDays,
    });
    await globalDoomerboard.replace(ctx, aggregateValue, {
      key: score.tokenScore,
      namespace: score.boardKey,
    });
  },
});
