import type { GenericId } from "convex/values";

import type { MutationCtx } from "../_generated/server";
import { resolveActiveDevice, tokenmaxxerForSubject } from "./identity";
import { rateLimiter } from "./rateLimits";
import { recomputeScores } from "./scores";
import {
  assertUsageSnapshot,
  rankingDayAt,
  subtractRankingDays,
  type UsageSnapshot,
} from "./values";

async function upsertDailyUsage(
  ctx: MutationCtx,
  tokenmaxxerId: GenericId<"tokenmaxxers">,
  snapshot: UsageSnapshot,
) {
  const rows = await ctx.db
    .query("userDailyUsage")
    .withIndex("by_tokenmaxxer_id", (q) => q.eq("tokenmaxxerId", tokenmaxxerId))
    .take(1_000);
  const existing = rows.find(
    (row) => row.provider === snapshot.provider && row.rankingDay === snapshot.rankingDay,
  );
  const values = {
    costIsComplete:
      snapshot.apiEquivalentCostMicros !== null || snapshot.observedTokens === 0,
    observedTokens: snapshot.observedTokens,
    updatedAt: Date.now(),
  };

  if (existing) {
    await ctx.db.patch(existing._id, {
      ...values,
      apiEquivalentCostMicros: snapshot.apiEquivalentCostMicros ?? undefined,
    });
  } else {
    await ctx.db.insert("userDailyUsage", {
      ...values,
      ...(snapshot.apiEquivalentCostMicros === null
        ? {}
        : { apiEquivalentCostMicros: snapshot.apiEquivalentCostMicros }),
      provider: snapshot.provider,
      rankingDay: snapshot.rankingDay,
      tokenmaxxerId,
    });
  }
}

export async function applyUsageSnapshots(
  ctx: MutationCtx,
  authSubject: string,
  installationId: string,
  snapshots: UsageSnapshot[],
) {
  if (snapshots.length === 0 || snapshots.length > 62) {
    throw new Error("sync must contain between 1 and 62 daily provider snapshots");
  }
  if (installationId.length < 16 || installationId.length > 128) {
    throw new Error("installationId must be an opaque 16-128 character identifier");
  }

  await rateLimiter.limit(ctx, "syncDailyUsage", {
    count: snapshots.length,
    key: authSubject,
    throws: true,
  });

  const tokenmaxxer = await tokenmaxxerForSubject(ctx, authSubject);
  if (!tokenmaxxer) {
    throw new Error("create a TouchGrass identity before synchronizing usage");
  }
  const device = await resolveActiveDevice(ctx, tokenmaxxer._id, installationId);
  const buckets = await ctx.db
    .query("usageBuckets")
    .withIndex("by_device_id", (q) => q.eq("deviceId", device._id))
    .take(1_000);

  const today = rankingDayAt();
  const oldestAcceptedDay = subtractRankingDays(today, 60);
  let changed = 0;
  for (const snapshot of snapshots) {
    assertUsageSnapshot(snapshot);
    if (snapshot.rankingDay > today || snapshot.rankingDay < oldestAcceptedDay) {
      throw new Error("rankingDay must be within the last 60 UTC days");
    }
    const existing = buckets.find(
      (bucket) =>
        bucket.provider === snapshot.provider && bucket.rankingDay === snapshot.rankingDay,
    );
    if (existing && snapshot.revision <= existing.revision) {
      continue;
    }

    const values = {
      coverage: snapshot.coverage,
      observedAt: snapshot.observedAt,
      observedTokens: snapshot.observedTokens,
      revision: snapshot.revision,
      source: snapshot.source,
      syncedAt: Date.now(),
    };
    if (existing) {
      await ctx.db.patch(existing._id, {
        ...values,
        apiEquivalentCostMicros: snapshot.apiEquivalentCostMicros ?? undefined,
        priceBasisVersion: snapshot.priceBasisVersion ?? undefined,
      });
    } else {
      await ctx.db.insert("usageBuckets", {
        ...values,
        ...(snapshot.apiEquivalentCostMicros === null
          ? {}
          : { apiEquivalentCostMicros: snapshot.apiEquivalentCostMicros }),
        deviceId: device._id,
        ...(snapshot.priceBasisVersion === null
          ? {}
          : { priceBasisVersion: snapshot.priceBasisVersion }),
        provider: snapshot.provider,
        rankingDay: snapshot.rankingDay,
        tokenmaxxerId: tokenmaxxer._id,
      });
    }
    await upsertDailyUsage(ctx, tokenmaxxer._id, snapshot);
    changed += 1;
  }

  await ctx.db.patch(tokenmaxxer._id, { lastSyncedAt: Date.now() });
  const overview = await recomputeScores(ctx, tokenmaxxer._id, today);
  return { changedBuckets: changed, overview };
}
