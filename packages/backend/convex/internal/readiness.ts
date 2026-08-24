import type { PaginationResult } from "convex/server";
import { v } from "convex/values";

import { components, internal } from "../_generated/api";
import {
  env,
  internalAction,
  internalMutation,
  internalQuery,
  type MutationCtx,
} from "../_generated/server";
import { doomerboard, doomerboardKey } from "../model/doomerboard";
import { markDoomerboardChanged } from "../model/doomerboardVersion";
import { BACKEND_POLICY_VERSION, backendPolicy } from "../model/policy";
import { rateLimiter } from "../model/rateLimits";
import { validTouchGrassId } from "../model/touchGrassId";
import { BOARD_KEY_VERSION } from "../model/values";
import { migrations } from "./migrations";
import { deployedBackendBinding } from "./readinessDeployment";

const canaryDisplayNamePattern =
  /^Readiness Canary [23456789ABCDEFGHJKMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz]{12}$/u;

const runtimeBindingValidator = v.object({
  boardKeyVersion: v.string(),
  commit: v.string(),
  lockHash: v.string(),
  policyVersion: v.string(),
  schemaHash: v.string(),
});

const requiredEnvironmentValidator = v.object({
  backendBinding: v.boolean(),
  betterAuthSecret: v.boolean(),
  productionDeployment: v.boolean(),
});

const metadataValidator = v.object({
  productionDeployment: v.string(),
  requiredEnvironment: requiredEnvironmentValidator,
  runtimeBinding: runtimeBindingValidator,
});

const doomerboardInvariantValidator = v.object({
  aggregateEntries: v.number(),
  extraEntries: v.number(),
  invalidEntries: v.number(),
  mismatchedEntries: v.number(),
  missingEntries: v.number(),
  publicScores: v.number(),
});

const healthInspectionValidator = metadataValidator.extend({
  canaryResidue: v.object({ markers: v.number() }),
  componentChecks: v.object({
    betterAuth: v.boolean(),
    doomerboard: v.boolean(),
    migrations: v.boolean(),
    rateLimiter: v.boolean(),
  }),
  deviceMigration: v.object({
    devices: v.number(),
    missingCompletionFields: v.number(),
  }),
  doomerboardInvariant: doomerboardInvariantValidator,
});

const cleanupResult = v.object({
  aggregateEntriesRemoved: v.number(),
  appRecordsRemoved: v.number(),
  authRecordsRemoved: v.number(),
  cleanupComplete: v.boolean(),
  rateLimitKeysReset: v.number(),
});

export const registerCanary = internalMutation({
  args: { displayName: v.string(), touchGrassId: v.string() },
  returns: v.null(),
  handler: async (ctx, args) => {
    if (!canaryDisplayNamePattern.test(args.displayName) || !validTouchGrassId(args.touchGrassId)) {
      throw new Error("Readiness canary marker rejected");
    }
    const [existingCanary, existingProfile, existingAuthUser] = await Promise.all([
      ctx.db
        .query("readinessCanaries")
        .withIndex("by_touch_grass_id", (query) => query.eq("touchGrassId", args.touchGrassId))
        .unique(),
      ctx.db
        .query("tokenmaxxers")
        .withIndex("by_public_id", (query) => query.eq("publicId", args.touchGrassId))
        .unique(),
      ctx.runQuery(components.betterAuth.adapter.findOne, {
        model: "user",
        select: ["_id"],
        where: [{ field: "username", value: args.touchGrassId }],
      }),
    ]);
    if (existingCanary || existingProfile || existingAuthUser) {
      throw new Error("Readiness canary marker rejected");
    }
    const createdAt = Date.now();
    const expiresAt = createdAt + backendPolicy.authentication.canaryLifetimeMs;
    const canaryMarkerId = await ctx.db.insert("readinessCanaries", {
      createdAt,
      displayName: args.displayName,
      expiresAt,
      touchGrassId: args.touchGrassId,
    });
    await ctx.scheduler.runAt(expiresAt, internal.internal.readiness.expireCanary, {
      canaryMarkerId,
    });
    return null;
  },
});

function boundedRows<T>(rows: T[], label: string) {
  if (rows.length > backendPolicy.authentication.canaryRelatedRowsPerTable) {
    throw new Error(`Readiness canary ${label} cleanup limit exceeded`);
  }
  return rows;
}

function present(value: string | undefined) {
  return typeof value === "string" && value.trim().length > 0;
}

export const metadata = internalQuery({
  args: {},
  returns: metadataValidator,
  handler: () => ({
    productionDeployment: deployedBackendBinding.productionDeployment,
    requiredEnvironment: {
      backendBinding:
        deployedBackendBinding.commit !== "unbound" &&
        deployedBackendBinding.lockHash !== "unbound" &&
        deployedBackendBinding.schemaHash !== "unbound" &&
        deployedBackendBinding.boardKeyVersion === BOARD_KEY_VERSION &&
        deployedBackendBinding.policyVersion === BACKEND_POLICY_VERSION,
      betterAuthSecret: present(env.BETTER_AUTH_SECRET),
      productionDeployment: deployedBackendBinding.productionDeployment !== "unbound",
    },
    runtimeBinding: {
      boardKeyVersion: BOARD_KEY_VERSION,
      commit: deployedBackendBinding.commit,
      lockHash: deployedBackendBinding.lockHash,
      policyVersion: BACKEND_POLICY_VERSION,
      schemaHash: deployedBackendBinding.schemaHash,
    },
  }),
});

export const inspectHealth = internalAction({
  args: {},
  returns: healthInspectionValidator,
  handler: async (ctx) => {
    const metadataResult: {
      productionDeployment: string;
      requiredEnvironment: {
        backendBinding: boolean;
        betterAuthSecret: boolean;
        productionDeployment: boolean;
      };
      runtimeBinding: {
        boardKeyVersion: string;
        commit: string;
        lockHash: string;
        policyVersion: string;
        schemaHash: string;
      };
    } = await ctx.runQuery(internal.internal.readiness.metadata, {});
    await ctx.runQuery(components.betterAuth.adapter.findOne, {
      model: "user",
      select: ["_id"],
      where: [{ field: "username", value: "TG-222222" }],
    });
    const migrationStatuses = await migrations.getStatus(ctx, {
      migrations: [
        "internal/migrations:backfillDoomerboard",
        "internal/migrations:backfillDeviceUsageCompletion",
      ],
    });
    await rateLimiter.getValue(ctx, "syncDailyUsage", {
      key: "backend-readiness-component-probe",
    });
    const doomerboardInvariant: {
      aggregateEntries: number;
      extraEntries: number;
      invalidEntries: number;
      mismatchedEntries: number;
      missingEntries: number;
      publicScores: number;
    } = await ctx.runAction(internal.internal.doomerboardInvariant.check, {});
    let cursor: string | null = null;
    let devices = 0;
    let missingCompletionFields = 0;
    let complete = false;
    for (let page = 0; page < backendPolicy.health.maxPages && !complete; page += 1) {
      const result: PaginationResult<{ hasCompletionField: boolean }> = await ctx.runQuery(
        internal.internal.readinessPage.devices,
        {
          paginationOpts: {
            cursor,
            maximumRowsRead: backendPolicy.health.pageSize,
            numItems: backendPolicy.health.pageSize,
          },
        },
      );
      devices += result.page.length;
      missingCompletionFields += result.page.filter((device) => !device.hasCompletionField).length;
      cursor = result.continueCursor;
      complete = result.isDone;
    }
    if (!complete) throw new Error("Production health Device scan exceeded its bounded policy");
    let canaryCursor: string | null = null;
    let canaryMarkers = 0;
    let canaryScanComplete = false;
    for (let page = 0; page < backendPolicy.health.maxPages && !canaryScanComplete; page += 1) {
      const result: PaginationResult<null> = await ctx.runQuery(
        internal.internal.readinessPage.canaries,
        {
          paginationOpts: {
            cursor: canaryCursor,
            maximumRowsRead: backendPolicy.health.pageSize,
            numItems: backendPolicy.health.pageSize,
          },
        },
      );
      canaryMarkers += result.page.length;
      canaryCursor = result.continueCursor;
      canaryScanComplete = result.isDone;
    }
    if (!canaryScanComplete) {
      throw new Error("Production health canary residue scan exceeded its bounded policy");
    }
    return {
      ...metadataResult,
      canaryResidue: { markers: canaryMarkers },
      componentChecks: {
        betterAuth: true,
        doomerboard: true,
        migrations: migrationStatuses.every(
          (status) => status.isDone && status.state === "success" && !status.error,
        ),
        rateLimiter: true,
      },
      deviceMigration: { devices, missingCompletionFields },
      doomerboardInvariant,
    };
  },
});

async function deleteAuthRows(ctx: MutationCtx, model: "account" | "session", authSubject: string) {
  const limit = backendPolicy.authentication.canaryAuthRowsPerModel;
  const result = await ctx.runMutation(components.betterAuth.adapter.deleteMany, {
    input: {
      model,
      where: [{ field: "userId", value: authSubject }],
    },
    paginationOpts: {
      cursor: null,
      maximumRowsRead: limit + 1,
      numItems: limit + 1,
    },
  });
  if (!result.isDone || result.count > limit) {
    throw new Error(`Readiness canary ${model} cleanup limit exceeded`);
  }
  return result.count;
}

async function deleteAuthUser(ctx: MutationCtx, authSubject: string) {
  const result = await ctx.runMutation(components.betterAuth.adapter.deleteMany, {
    input: {
      model: "user",
      where: [{ field: "_id", value: authSubject }],
    },
    paginationOpts: {
      cursor: null,
      maximumRowsRead: 2,
      numItems: 2,
    },
  });
  if (!result.isDone || result.count > 1) {
    throw new Error("Readiness canary user cleanup invariant failed");
  }
  return result.count;
}

async function cleanupCanaryHandler(
  ctx: MutationCtx,
  args: { displayName: string; touchGrassId: string },
) {
  const canaryMarker = await ctx.db
    .query("readinessCanaries")
    .withIndex("by_touch_grass_id", (query) => query.eq("touchGrassId", args.touchGrassId))
    .unique();
  const tokenmaxxer = await ctx.db
    .query("tokenmaxxers")
    .withIndex("by_public_id", (query) => query.eq("publicId", args.touchGrassId))
    .unique();
  const authUser = (await ctx.runQuery(components.betterAuth.adapter.findOne, {
    model: "user",
    select: ["_id", "name"],
    where: [{ field: "username", value: args.touchGrassId }],
  })) as { _id?: unknown; name?: unknown } | null;
  const componentAuthSubject = typeof authUser?._id === "string" ? authUser._id : null;
  if (
    tokenmaxxer &&
    componentAuthSubject !== null &&
    tokenmaxxer.authSubject !== componentAuthSubject
  ) {
    throw new Error("Readiness canary Profile ownership invariant failed");
  }
  if (
    !canaryDisplayNamePattern.test(args.displayName) ||
    (canaryMarker !== null && canaryMarker.displayName !== args.displayName) ||
    (tokenmaxxer !== null && tokenmaxxer.displayName !== args.displayName) ||
    (authUser !== null && authUser.name !== args.displayName)
  ) {
    throw new Error("Readiness canary cleanup marker rejected");
  }
  const authSubject = tokenmaxxer?.authSubject ?? componentAuthSubject;

  if (canaryMarker === null) {
    if (tokenmaxxer !== null || authUser !== null) {
      throw new Error("Readiness canary cleanup marker rejected");
    }
    return {
      aggregateEntriesRemoved: 0,
      appRecordsRemoved: 0,
      authRecordsRemoved: 0,
      cleanupComplete: true,
      rateLimitKeysReset: 0,
    };
  }

  let aggregateEntriesRemoved = 0;
  let appRecordsRemoved = 0;
  let rateLimitKeysReset = 0;
  if (tokenmaxxer) {
    const limit = backendPolicy.authentication.canaryRelatedRowsPerTable + 1;
    const devices = boundedRows(
      await ctx.db
        .query("devices")
        .withIndex("by_tokenmaxxer_id", (query) => query.eq("tokenmaxxerId", tokenmaxxer._id))
        .take(limit),
      "Device",
    );
    const recoveryAttempts = boundedRows(
      await ctx.db
        .query("profileRecoveryAttempts")
        .withIndex("by_tokenmaxxer_id", (query) => query.eq("tokenmaxxerId", tokenmaxxer._id))
        .take(limit),
      "recovery attempt",
    );
    const usageBuckets = boundedRows(
      await ctx.db
        .query("usageBuckets")
        .withIndex("by_tokenmaxxer_id_and_provider_and_ranking_day", (query) =>
          query.eq("tokenmaxxerId", tokenmaxxer._id),
        )
        .take(limit),
      "Usage Bucket",
    );
    const transferBoundaries = boundedRows(
      await ctx.db
        .query("usageTransferBoundaries")
        .withIndex("by_tokenmaxxer_id_and_provider_and_ranking_day", (query) =>
          query.eq("tokenmaxxerId", tokenmaxxer._id),
        )
        .take(limit),
      "transfer boundary",
    );
    const dailyUsages = boundedRows(
      await ctx.db
        .query("userDailyUsage")
        .withIndex("by_tokenmaxxer_id", (query) => query.eq("tokenmaxxerId", tokenmaxxer._id))
        .take(limit),
      "Daily Usage",
    );
    const publicUsages = boundedRows(
      await ctx.db
        .query("publicUsages")
        .withIndex("by_tokenmaxxer_id", (query) => query.eq("tokenmaxxerId", tokenmaxxer._id))
        .take(limit),
      "Public Usage",
    );
    const outgoingEdges = boundedRows(
      await ctx.db
        .query("addedTokenmaxxers")
        .withIndex("by_owner_id", (query) => query.eq("ownerId", tokenmaxxer._id))
        .take(limit),
      "outgoing My Tokenmaxxers",
    );
    const providerSettings = (
      await Promise.all(
        devices.map((device) =>
          ctx.db
            .query("deviceProviderSettings")
            .withIndex("by_device_id", (query) => query.eq("deviceId", device._id))
            .unique(),
        ),
      )
    ).filter((row) => row !== null);
    const correctionAudits = boundedRows(
      (
        await Promise.all(
          usageBuckets.map((bucket) =>
            ctx.db
              .query("usageCorrectionAudits")
              .withIndex("by_bucket_id_and_revision", (query) => query.eq("bucketId", bucket._id))
              .take(limit),
          ),
        )
      ).flat(),
      "correction audit",
    );

    await rateLimiter.reset(ctx, "successfulProfileRecovery", {
      key: String(tokenmaxxer._id),
    });
    rateLimitKeysReset += 1;
    for (const device of devices) {
      await rateLimiter.reset(ctx, "syncDailyUsage", {
        key: `${tokenmaxxer._id}:${device._id}:${device.generation}`,
      });
      rateLimitKeysReset += 1;
    }

    for (const publicUsage of publicUsages) {
      await doomerboard.deleteIfExists(ctx, {
        id: publicUsage._id,
        key: publicUsage.tokenScore,
        namespace: publicUsage.boardKey,
      });
      await doomerboard.deleteIfExists(ctx, {
        id: publicUsage._id,
        key: doomerboardKey(publicUsage.tokenScore, publicUsage.touchGrassId),
        namespace: publicUsage.boardKey,
      });
      aggregateEntriesRemoved += 1;
    }
    const appRows = [
      ...correctionAudits,
      ...providerSettings,
      ...outgoingEdges,
      ...transferBoundaries,
      ...usageBuckets,
      ...dailyUsages,
      ...publicUsages,
      ...recoveryAttempts,
      ...devices,
    ];
    const deletedIds = new Set<string>();
    for (const row of appRows) {
      if (deletedIds.has(row._id)) continue;
      await ctx.db.delete(row._id);
      deletedIds.add(row._id);
      appRecordsRemoved += 1;
    }
    await ctx.db.delete(tokenmaxxer._id);
    appRecordsRemoved += 1;
    if (publicUsages.length > 0) await markDoomerboardChanged(ctx);
  }

  let authRecordsRemoved = 0;
  if (authSubject !== null) {
    authRecordsRemoved += await deleteAuthRows(ctx, "session", authSubject);
    authRecordsRemoved += await deleteAuthRows(ctx, "account", authSubject);
    authRecordsRemoved += await deleteAuthUser(ctx, authSubject);
  }
  await ctx.db.delete(canaryMarker._id);
  appRecordsRemoved += 1;
  const remainingProfile = await ctx.db
    .query("tokenmaxxers")
    .withIndex("by_public_id", (query) => query.eq("publicId", args.touchGrassId))
    .unique();
  const remainingAuthUser = await ctx.runQuery(components.betterAuth.adapter.findOne, {
    model: "user",
    select: ["_id"],
    where: [{ field: "username", value: args.touchGrassId }],
  });
  const remainingCanaryMarker = await ctx.db
    .query("readinessCanaries")
    .withIndex("by_touch_grass_id", (query) => query.eq("touchGrassId", args.touchGrassId))
    .unique();
  return {
    aggregateEntriesRemoved,
    appRecordsRemoved,
    authRecordsRemoved,
    cleanupComplete:
      remainingProfile === null && remainingAuthUser === null && remainingCanaryMarker === null,
    rateLimitKeysReset,
  };
}

export const cleanupCanary = internalMutation({
  args: { displayName: v.string(), touchGrassId: v.string() },
  returns: cleanupResult,
  handler: cleanupCanaryHandler,
});

export const expireCanary = internalMutation({
  args: { canaryMarkerId: v.id("readinessCanaries") },
  returns: cleanupResult,
  handler: async (ctx, args) => {
    const marker = await ctx.db.get(args.canaryMarkerId);
    if (marker === null) {
      return {
        aggregateEntriesRemoved: 0,
        appRecordsRemoved: 0,
        authRecordsRemoved: 0,
        cleanupComplete: true,
        rateLimitKeysReset: 0,
      };
    }
    if (marker.expiresAt > Date.now()) {
      await ctx.scheduler.runAt(marker.expiresAt, internal.internal.readiness.expireCanary, args);
      return {
        aggregateEntriesRemoved: 0,
        appRecordsRemoved: 0,
        authRecordsRemoved: 0,
        cleanupComplete: false,
        rateLimitKeysReset: 0,
      };
    }
    return cleanupCanaryHandler(ctx, marker);
  },
});
