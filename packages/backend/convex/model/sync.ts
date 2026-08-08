import type { GenericId } from "convex/values";

import type { Doc } from "../_generated/dataModel";
import type { MutationCtx } from "../_generated/server";
import { rateLimiter } from "./rateLimits";
import { requireActiveDevice, type AuthUserReference } from "./profile";
import { recomputeScores } from "./scores";
import {
  assertUsageSnapshot,
  rankingDayAt,
  type Provider,
  type UsageSnapshot,
} from "./values";

export type UsageAcknowledgement = {
  outcome: "committed" | "idempotent" | "stale";
  provider: Provider;
  rankingDay: string;
  revision: number;
};

type SnapshotPlan = {
  acknowledgement: UsageAcknowledgement;
  correctionAudit: CorrectionLineage | null;
  existing: Doc<"usageBuckets"> | null;
  snapshot: UsageSnapshot;
};

type CorrectionLineage = {
  reason: NonNullable<UsageSnapshot["correctionReason"]>;
  revision: number;
};

async function upsertDailyUsage(
  ctx: MutationCtx,
  tokenmaxxerId: GenericId<"tokenmaxxers">,
  snapshot: UsageSnapshot,
) {
  const existing = await ctx.db
    .query("userDailyUsage")
    .withIndex("by_tokenmaxxer_id_and_provider_and_ranking_day", (q) =>
      q
        .eq("tokenmaxxerId", tokenmaxxerId)
        .eq("provider", snapshot.provider)
        .eq("rankingDay", snapshot.rankingDay),
    )
    .unique();
  const apiEquivalentCostMicros = snapshot.apiEquivalentCost?.micros;
  const costIsComplete =
    snapshot.observedTokens === 0 ||
    (snapshot.apiEquivalentCost !== null &&
      (snapshot.apiEquivalentCost.quality !== "modeled" ||
        snapshot.apiEquivalentCost.coveragePercent === 100));
  const values = {
    costIsComplete,
    observedTokens: snapshot.observedTokens,
    updatedAt: Date.now(),
  };

  if (existing) {
    await ctx.db.patch(existing._id, {
      ...values,
      apiEquivalentCost: snapshot.apiEquivalentCost ?? undefined,
      apiEquivalentCostMicros,
    });
    return;
  }
  await ctx.db.insert("userDailyUsage", {
    ...values,
    ...(snapshot.apiEquivalentCost === null
      ? {}
      : { apiEquivalentCost: snapshot.apiEquivalentCost }),
    ...(apiEquivalentCostMicros === undefined ? {} : { apiEquivalentCostMicros }),
    provider: snapshot.provider,
    rankingDay: snapshot.rankingDay,
    tokenmaxxerId,
  });
}

function assertBatchSize(snapshots: UsageSnapshot[]) {
  if (snapshots.length === 0 || snapshots.length > 62) {
    throw new Error("sync must contain between 1 and 62 Daily Usage Aggregates");
  }
}

function validateBatch(snapshots: UsageSnapshot[], today: string, now: number) {
  const keys = new Set<string>();
  for (const snapshot of snapshots) {
    assertUsageSnapshot(snapshot, today, now);
    const key = `${snapshot.provider}:${snapshot.rankingDay}`;
    if (keys.has(key)) {
      throw new Error("sync must contain at most one snapshot per provider and Ranking Day");
    }
    keys.add(key);
  }
}

function snapshotCorrectionLineage(
  snapshot: UsageSnapshot,
): CorrectionLineage | null {
  if (
    snapshot.correctionReason === null ||
    snapshot.correctionRevision === null
  ) {
    return null;
  }
  return {
    reason: snapshot.correctionReason,
    revision: snapshot.correctionRevision,
  };
}

function storedCorrectionLineage(
  existing: Doc<"usageBuckets"> | null,
): CorrectionLineage | null {
  if (!existing) return null;
  if (
    existing.lastCorrectionReason === undefined &&
    existing.lastCorrectionRevision === undefined
  ) {
    return null;
  }
  if (
    existing.lastCorrectionReason === undefined ||
    existing.lastCorrectionRevision === undefined
  ) {
    throw new Error("stored correction lineage is incomplete");
  }
  return {
    reason: existing.lastCorrectionReason,
    revision: existing.lastCorrectionRevision,
  };
}

function assertCompatibleFinalEvidence(
  lineage: CorrectionLineage,
  snapshot: UsageSnapshot,
) {
  if (
    lineage.reason === "provider-replacement" &&
    snapshot.evidenceBasis !== "provider-reported"
  ) {
    throw new Error(
      "provider replacement requires provider-reported final evidence",
    );
  }
  if (
    lineage.reason === "parser-correction" &&
    snapshot.evidenceBasis !== "locally-derived"
  ) {
    throw new Error("parser correction requires locally-derived final evidence");
  }
}

function assertNewCorrectionProvenance(
  existing: Doc<"usageBuckets"> | null,
  lineage: CorrectionLineage,
  snapshot: UsageSnapshot,
) {
  assertCompatibleFinalEvidence(lineage, snapshot);
  if (lineage.reason === "provider-replacement") {
    if (existing && existing.evidenceBasis !== "locally-derived") {
      throw new Error(
        "provider replacement requires locally-derived to provider-reported evidence",
      );
    }
    return;
  }
  if (existing && existing.evidenceBasis !== "locally-derived") {
    throw new Error(
      "parser correction requires locally-derived evidence on both revisions",
    );
  }
}

function planCorrectionLineage(
  existing: Doc<"usageBuckets"> | null,
  snapshot: UsageSnapshot,
): CorrectionLineage | null {
  const incoming = snapshotCorrectionLineage(snapshot);
  const stored = storedCorrectionLineage(existing);
  if (!incoming) {
    if (existing && snapshot.observedTokens < existing.observedTokens) {
      throw new Error("a lower observed token total requires correction provenance");
    }
    return null;
  }
  if (
    stored &&
    stored.reason === incoming.reason &&
    stored.revision === incoming.revision
  ) {
    assertCompatibleFinalEvidence(incoming, snapshot);
    if (existing && snapshot.observedTokens < existing.observedTokens) {
      throw new Error("a known correction lineage cannot explain another decrease");
    }
    return null;
  }
  if (existing && incoming.revision <= existing.revision) {
    throw new Error("correction lineage is retroactive");
  }
  assertNewCorrectionProvenance(existing, incoming, snapshot);
  return incoming;
}

async function planSnapshots(
  ctx: MutationCtx,
  deviceId: GenericId<"devices">,
  snapshots: UsageSnapshot[],
) {
  const plans: SnapshotPlan[] = [];
  for (const snapshot of snapshots) {
    const existing = await ctx.db
      .query("usageBuckets")
      .withIndex("by_device_id_and_provider_and_ranking_day", (q) =>
        q
          .eq("deviceId", deviceId)
          .eq("provider", snapshot.provider)
          .eq("rankingDay", snapshot.rankingDay),
      )
      .unique();
    if (existing && snapshot.revision < existing.revision) {
      plans.push({
        acknowledgement: {
          outcome: "stale",
          provider: snapshot.provider,
          rankingDay: snapshot.rankingDay,
          revision: existing.revision,
        },
        correctionAudit: null,
        existing,
        snapshot,
      });
      continue;
    }
    if (existing && snapshot.revision === existing.revision) {
      plans.push({
        acknowledgement: {
          outcome: "idempotent",
          provider: snapshot.provider,
          rankingDay: snapshot.rankingDay,
          revision: existing.revision,
        },
        correctionAudit: null,
        existing,
        snapshot,
      });
      continue;
    }
    const correctionAudit = planCorrectionLineage(existing, snapshot);
    plans.push({
      acknowledgement: {
        outcome: "committed",
        provider: snapshot.provider,
        rankingDay: snapshot.rankingDay,
        revision: snapshot.revision,
      },
      correctionAudit,
      existing,
      snapshot,
    });
  }
  return plans;
}

async function commitSnapshot(
  ctx: MutationCtx,
  tokenmaxxerId: GenericId<"tokenmaxxers">,
  deviceId: GenericId<"devices">,
  plan: SnapshotPlan,
) {
  const { correctionAudit, existing, snapshot } = plan;
  const lineage = snapshotCorrectionLineage(snapshot);
  const values = {
    apiEquivalentCost: snapshot.apiEquivalentCost,
    correctionReason: snapshot.correctionReason,
    correctionRevision: snapshot.correctionRevision,
    ...(lineage
      ? {
          lastCorrectionReason: lineage.reason,
          lastCorrectionRevision: lineage.revision,
        }
      : {}),
    coverage: snapshot.coverage,
    evidenceBasis: snapshot.evidenceBasis,
    observedAt: snapshot.observedAt,
    observedTokens: snapshot.observedTokens,
    revision: snapshot.revision,
    syncedAt: Date.now(),
  };
  let bucketId: GenericId<"usageBuckets">;
  if (existing) {
    await ctx.db.patch(existing._id, {
      ...values,
      apiEquivalentCostMicros: undefined,
      priceBasisVersion: undefined,
      source: undefined,
    });
    bucketId = existing._id;
  } else {
    bucketId = await ctx.db.insert("usageBuckets", {
      ...values,
      deviceId,
      provider: snapshot.provider,
      rankingDay: snapshot.rankingDay,
      tokenmaxxerId,
    });
  }
  if (correctionAudit) {
    await ctx.db.insert("usageCorrectionAudits", {
      bucketId,
      createdAt: Date.now(),
      deviceId,
      provider: snapshot.provider,
      rankingDay: snapshot.rankingDay,
      reason: correctionAudit.reason,
      revision: correctionAudit.revision,
      tokenmaxxerId,
    });
  }
  await upsertDailyUsage(ctx, tokenmaxxerId, snapshot);
}

export async function applyUsageSnapshots(
  ctx: MutationCtx,
  authUser: AuthUserReference,
  installationCredential: string,
  activeMacGeneration: number,
  snapshots: UsageSnapshot[],
) {
  const now = Date.now();
  const today = rankingDayAt(now);
  assertBatchSize(snapshots);
  const { device, tokenmaxxer } = await requireActiveDevice(
    ctx,
    authUser,
    installationCredential,
    activeMacGeneration,
  );
  await rateLimiter.limit(ctx, "syncDailyUsage", {
    count: snapshots.length,
    key: `${tokenmaxxer._id}:${device._id}:${activeMacGeneration}`,
    throws: true,
  });
  validateBatch(snapshots, today, now);
  const plans = await planSnapshots(ctx, device._id, snapshots);
  const committed = plans.filter(
    ({ acknowledgement }) => acknowledgement.outcome === "committed",
  );
  for (const plan of committed) {
    await commitSnapshot(ctx, tokenmaxxer._id, device._id, plan);
  }
  if (committed.length > 0) {
    const syncedAt = Date.now();
    await ctx.db.patch(device._id, { lastSeenAt: syncedAt });
    await ctx.db.patch(tokenmaxxer._id, { lastSyncedAt: syncedAt });
    await recomputeScores(ctx, tokenmaxxer._id, today);
  }
  return plans.map(({ acknowledgement }) => acknowledgement);
}
