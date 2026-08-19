import type { GenericId } from "convex/values";

import type { Doc } from "../_generated/dataModel";
import type { MutationCtx } from "../_generated/server";
import { rateLimiter } from "./rateLimits";
import { requireActiveDevice, type AuthUserReference } from "./profile";
import { calculateScore, recomputeScores } from "./scores";
import {
  assertUsageSnapshot,
  rankingDayAt,
  subtractRankingDays,
  type Provider,
  type UsageSnapshot,
} from "./values";

export type UsageAcknowledgement = {
  outcome: "committed" | "conflict" | "idempotent" | "stale";
  provider: Provider;
  rankingDay: string;
  revision: number;
};

export type ProviderSettingsAcknowledgement = {
  outcome: "committed" | "idempotent" | "stale";
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

// One initial generation plus the transfer policy limit of three per hour.
const MAX_ACTIVE_MAC_SEGMENTS_PER_PROVIDER_DAY = 73;
const MAX_DEVICE_USAGE_BUCKETS = 120;
const MAX_TRANSFER_DAY_CARRYOVERS = 2;
const PROFILE_BACKFILL_DAYS = 30;
const RETAINED_USAGE_DAYS = 60;

async function upsertDailyUsage(
  ctx: MutationCtx,
  tokenmaxxerId: GenericId<"tokenmaxxers">,
  snapshot: Pick<UsageSnapshot, "provider" | "rankingDay">,
  costUnavailable: boolean,
) {
  const segments = await ctx.db
    .query("usageBuckets")
    .withIndex("by_tokenmaxxer_id_and_provider_and_ranking_day", (q) =>
      q
        .eq("tokenmaxxerId", tokenmaxxerId)
        .eq("provider", snapshot.provider)
        .eq("rankingDay", snapshot.rankingDay),
    )
    .take(MAX_ACTIVE_MAC_SEGMENTS_PER_PROVIDER_DAY + 1);
  if (segments.length > MAX_ACTIVE_MAC_SEGMENTS_PER_PROVIDER_DAY) {
    throw new Error("Daily Usage has too many Active Mac segments");
  }
  const dailyUsage = calculateScore(
    segments,
    snapshot.provider,
    1,
    snapshot.rankingDay,
  );
  const existing = await ctx.db
    .query("userDailyUsage")
    .withIndex("by_tokenmaxxer_id_and_provider_and_ranking_day", (q) =>
      q
        .eq("tokenmaxxerId", tokenmaxxerId)
        .eq("provider", snapshot.provider)
        .eq("rankingDay", snapshot.rankingDay),
    )
    .unique();
  const values = {
    apiEquivalentCost: costUnavailable ? null : dailyUsage.apiEquivalentCost,
    observedTokens: dailyUsage.tokenScore,
    updatedAt: Date.now(),
  };

  if (existing) {
    await ctx.db.patch(existing._id, values);
    return;
  }
  await ctx.db.insert("userDailyUsage", {
    ...values,
    provider: snapshot.provider,
    rankingDay: snapshot.rankingDay,
    tokenmaxxerId,
  });
}

export async function freezeTransferDayUsage(
  ctx: MutationCtx,
  tokenmaxxerId: GenericId<"tokenmaxxers">,
  previousDeviceId: GenericId<"devices">,
  newDeviceId: GenericId<"devices">,
  transferDay: string,
) {
  const oldSegments = await ctx.db
    .query("usageBuckets")
    .withIndex("by_device_id", (query) =>
      query.eq("deviceId", previousDeviceId),
    )
    .take(MAX_DEVICE_USAGE_BUCKETS + 1);
  if (oldSegments.length > MAX_DEVICE_USAGE_BUCKETS) {
    throw new Error("Active Mac usage history is out of bounds");
  }
  const providerSettings = await ctx.db
    .query("deviceProviderSettings")
    .withIndex("by_device_id", (query) =>
      query.eq("deviceId", previousDeviceId),
    )
    .unique();
  if (
    providerSettings?.tokenmaxxerId !== undefined &&
    providerSettings.tokenmaxxerId !== tokenmaxxerId
  ) {
    throw new Error("Active Mac provider settings owner is invalid");
  }
  const affectedProviders = new Set<Provider>(
    providerSettings
      ? [
          ...(providerSettings.codexEnabled ? (["codex"] as const) : []),
          ...(providerSettings.claudeEnabled ? (["claude"] as const) : []),
        ]
      : (["codex", "claude"] as const),
  );
  for (const segment of oldSegments) {
    if (segment.rankingDay === transferDay && segment.coverage !== "partial") {
      await ctx.db.patch(segment._id, { coverage: "partial" });
      affectedProviders.add(segment.provider);
    }
  }
  for (const provider of affectedProviders) {
    const existingBoundary = await ctx.db
      .query("usageTransferBoundaries")
      .withIndex(
        "by_tokenmaxxer_id_and_provider_and_ranking_day",
        (query) =>
          query
            .eq("tokenmaxxerId", tokenmaxxerId)
            .eq("provider", provider)
            .eq("rankingDay", transferDay),
      )
      .unique();
    if (!existingBoundary) {
      await ctx.db.insert("usageTransferBoundaries", {
        createdAt: Date.now(),
        newDeviceId,
        previousDeviceId,
        provider,
        rankingDay: transferDay,
        tokenmaxxerId,
      });
    }
  }
  for (const provider of affectedProviders) {
    await upsertDailyUsage(
      ctx,
      tokenmaxxerId,
      { provider, rankingDay: transferDay },
      false,
    );
  }
  if (affectedProviders.size > 0) {
    await recomputeScores(ctx, tokenmaxxerId, transferDay);
  }
}

function assertBatchSize(
  snapshots: UsageSnapshot[],
  profileBackfillAnchor: string | null,
) {
  if (profileBackfillAnchor !== null && snapshots.length > 60) {
    throw new Error(
      "a Profile backfill must contain at most 60 Daily Usage Snapshots",
    );
  }
  if (
    snapshots.length > 62 ||
    (snapshots.length === 0 && profileBackfillAnchor === null)
  ) {
    throw new Error("sync must contain between 1 and 62 Daily Usage Snapshots");
  }
}

function assertProfileBackfillMarker(
  device: Doc<"devices">,
  profileBackfillAnchor: string | null,
) {
  if (profileBackfillAnchor === null) return;
  if (device.generation !== 1) {
    throw new Error("only the first Active Mac can complete a Profile backfill");
  }
  if (profileBackfillAnchor !== rankingDayAt(device.createdAt)) {
    throw new Error(
      "Profile backfill anchor must match the Active Mac creation UTC day",
    );
  }
}

function normalizedEnabledProviders(enabledProviders: Provider[]) {
  if (enabledProviders.length > 2) {
    throw new Error("provider settings contain too many providers");
  }
  const enabled = new Set(enabledProviders);
  if (enabled.size !== enabledProviders.length) {
    throw new Error("provider settings contain a duplicate provider");
  }
  return {
    claudeEnabled: enabled.has("claude"),
    codexEnabled: enabled.has("codex"),
  };
}

function assertProviderSettingsRevision(revision: number) {
  if (!Number.isSafeInteger(revision) || revision < 1) {
    throw new Error("provider settings revision is invalid");
  }
}

function assertTransferDayCarryover(
  snapshot: UsageSnapshot,
  device: Doc<"devices">,
  today: string,
  now: number,
) {
  if (
    !Number.isSafeInteger(device.createdAt) ||
    device.createdAt < 0 ||
    device.generation <= 1 ||
    snapshot.rankingDay !== rankingDayAt(device.createdAt) ||
    snapshot.rankingDay >= today ||
    snapshot.coverage !== "partial" ||
    snapshot.observedAt < device.createdAt
  ) {
    throw new Error(
      "a historical snapshot must be an Active Mac transfer carryover",
    );
  }
  assertUsageSnapshot(snapshot, snapshot.rankingDay, now);
  if (
    snapshot.observedTokens === 0 &&
    (snapshot.apiEquivalentCost !== null ||
      snapshot.correctionReason !== null ||
      snapshot.correctionRevision !== null ||
      snapshot.revision !== 1)
  ) {
    throw new Error(
      "a zero-token transfer carryover must use revision one and have no cost or correction",
    );
  }
}

function validateBatch(
  snapshots: UsageSnapshot[],
  device: Doc<"devices">,
  today: string,
  now: number,
  profileBackfillAnchor: string | null,
) {
  const keys = new Set<string>();
  let transferDayCarryovers = 0;
  let profileBackfillSnapshots = 0;
  const firstProfileBackfillDay =
    profileBackfillAnchor === null
      ? null
      : subtractRankingDays(profileBackfillAnchor, PROFILE_BACKFILL_DAYS - 1);
  for (const snapshot of snapshots) {
    if (
      profileBackfillAnchor !== null &&
      firstProfileBackfillDay !== null &&
      (snapshot.rankingDay < firstProfileBackfillDay ||
        snapshot.rankingDay > profileBackfillAnchor)
    ) {
      throw new Error(
        "a marked Profile backfill snapshot is outside the Profile window",
      );
    }
    if (snapshot.rankingDay === today) {
      assertUsageSnapshot(snapshot, today, now);
      if (
        device.generation > 1 &&
        snapshot.rankingDay === rankingDayAt(device.createdAt) &&
        snapshot.observedAt < device.createdAt
      ) {
        throw new Error(
          "transfer-day evidence must be observed after Active Mac activation",
        );
      }
    } else if (device.generation === 1) {
      const firstRetainedDay = subtractRankingDays(
        today,
        RETAINED_USAGE_DAYS - 1,
      );
      if (
        snapshot.rankingDay < firstRetainedDay ||
        snapshot.rankingDay >= today
      ) {
        throw new Error("historical snapshot is outside the retained UTC window");
      }
      assertUsageSnapshot(snapshot, snapshot.rankingDay, now, true);
      profileBackfillSnapshots += 1;
      if (profileBackfillSnapshots > 60) {
        throw new Error("sync contains too many Profile backfill snapshots");
      }
    } else {
      assertTransferDayCarryover(snapshot, device, today, now);
      transferDayCarryovers += 1;
      if (transferDayCarryovers > MAX_TRANSFER_DAY_CARRYOVERS) {
        throw new Error("sync contains too many Active Mac transfer carryovers");
      }
    }
    const key = `${snapshot.provider}:${snapshot.rankingDay}`;
    if (keys.has(key)) {
      throw new Error("sync must contain at most one snapshot per provider and Ranking Day");
    }
    keys.add(key);
  }
}

function assertHistoricalAdmission(
  plans: SnapshotPlan[],
  device: Doc<"devices">,
  today: string,
  profileBackfillAnchor: string | null,
) {
  if (device.generation !== 1) return;
  const backfillIsComplete =
    typeof device.usageBackfillCompletedAt === "number";
  const anchorDay = rankingDayAt(device.createdAt);
  const firstBackfillDay = subtractRankingDays(
    anchorDay,
    PROFILE_BACKFILL_DAYS - 1,
  );
  for (const plan of plans) {
    if (plan.snapshot.rankingDay === today) continue;
    if (
      !backfillIsComplete &&
      !plan.existing &&
      profileBackfillAnchor === null
    ) {
      throw new Error(
        "new Profile history requires an explicit backfill completion marker",
      );
    }
    if (backfillIsComplete && !plan.existing) {
      if (plan.snapshot.rankingDay <= anchorDay) {
        throw new Error(
          "a completed Profile backfill keeps original-window missing days closed",
        );
      }
      const rankingDayStart = Date.parse(
        `${plan.snapshot.rankingDay}T00:00:00.000Z`,
      );
      const rankingDayEnd = rankingDayStart + 24 * 60 * 60 * 1_000;
      if (
        plan.snapshot.observedAt < rankingDayStart ||
        plan.snapshot.observedAt >= rankingDayEnd
      ) {
        throw new Error(
          "a delayed post-anchor snapshot must use in-day observation evidence",
        );
      }
      continue;
    }
    if (!plan.existing) {
      if (
        plan.snapshot.rankingDay < firstBackfillDay ||
        plan.snapshot.rankingDay > anchorDay
      ) {
        throw new Error("new historical usage is outside the Profile window");
      }
    }
  }
}

function sameApiEquivalentCost(
  left: UsageSnapshot["apiEquivalentCost"],
  right: UsageSnapshot["apiEquivalentCost"],
) {
  if (left === null || right === null) return left === right;
  return (
    left.coveragePercent === right.coveragePercent &&
    left.micros === right.micros &&
    left.pricingBasis === right.pricingBasis &&
    left.quality === right.quality
  );
}

function sameUsageSnapshot(
  existing: Doc<"usageBuckets">,
  snapshot: UsageSnapshot,
) {
  return (
    existing.provider === snapshot.provider &&
    existing.rankingDay === snapshot.rankingDay &&
    existing.revision === snapshot.revision &&
    existing.observedTokens === snapshot.observedTokens &&
    sameApiEquivalentCost(existing.apiEquivalentCost, snapshot.apiEquivalentCost) &&
    existing.coverage === snapshot.coverage &&
    existing.evidenceBasis === snapshot.evidenceBasis &&
    existing.correctionReason === snapshot.correctionReason &&
    existing.correctionRevision === snapshot.correctionRevision &&
    existing.observedAt === snapshot.observedAt
  );
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
      const outcome = sameUsageSnapshot(existing, snapshot)
        ? "idempotent"
        : snapshot.observedAt < existing.observedAt ||
            snapshot.observedTokens < existing.observedTokens
          ? "conflict"
          : "stale";
      plans.push({
        acknowledgement: {
          outcome,
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
    if (existing && snapshot.observedAt < existing.observedAt) {
      plans.push({
        acknowledgement: {
          outcome: "conflict",
          provider: snapshot.provider,
          rankingDay: snapshot.rankingDay,
          revision: snapshot.revision,
        },
        correctionAudit: null,
        existing,
        snapshot,
      });
      continue;
    }
    if (
      existing?.evidenceBasis === "provider-reported" &&
      snapshot.evidenceBasis === "locally-derived"
    ) {
      plans.push({
        acknowledgement: {
          outcome: "stale",
          provider: snapshot.provider,
          rankingDay: snapshot.rankingDay,
          revision: snapshot.revision,
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
  costUnavailable: boolean,
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
    await ctx.db.patch(existing._id, values);
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
  await upsertDailyUsage(ctx, tokenmaxxerId, snapshot, costUnavailable);
}

export async function applyUsageSnapshots(
  ctx: MutationCtx,
  authUser: AuthUserReference,
  installationCredential: string,
  activeMacGeneration: number,
  snapshots: UsageSnapshot[],
  profileBackfillAnchor: string | null,
) {
  const now = Date.now();
  const today = rankingDayAt(now);
  assertBatchSize(snapshots, profileBackfillAnchor);
  const { device, tokenmaxxer } = await requireActiveDevice(
    ctx,
    authUser,
    installationCredential,
    activeMacGeneration,
  );
  assertProfileBackfillMarker(device, profileBackfillAnchor);
  await rateLimiter.limit(ctx, "syncDailyUsage", {
    count: Math.max(snapshots.length, 1),
    key: `${tokenmaxxer._id}:${device._id}:${activeMacGeneration}`,
    throws: true,
  });
  validateBatch(snapshots, device, today, now, profileBackfillAnchor);
  const acceptedSnapshots = snapshots.map((snapshot) =>
    device.generation > 1 && snapshot.rankingDay === rankingDayAt(device.createdAt)
      ? { ...snapshot, coverage: "partial" as const }
      : snapshot,
  );
  const plans = await planSnapshots(ctx, device._id, acceptedSnapshots);
  assertHistoricalAdmission(plans, device, today, profileBackfillAnchor);
  const committed = plans.filter(
    ({ acknowledgement }) => acknowledgement.outcome === "committed",
  );
  for (const plan of committed) {
    await commitSnapshot(
      ctx,
      tokenmaxxer._id,
      device._id,
      plan,
      device.generation > 1 && plan.snapshot.rankingDay !== today,
    );
  }
  if (committed.length > 0) {
    const syncedAt = Date.now();
    await ctx.db.patch(device._id, { lastSeenAt: syncedAt });
    await ctx.db.patch(tokenmaxxer._id, { lastSyncedAt: syncedAt });
    await recomputeScores(ctx, tokenmaxxer._id, today);
  }
  if (
    device.generation === 1 &&
    typeof device.usageBackfillCompletedAt !== "number" &&
    profileBackfillAnchor !== null
  ) {
    await ctx.db.patch(device._id, { usageBackfillCompletedAt: Date.now() });
  }
  return plans.map(({ acknowledgement }) => acknowledgement);
}

export async function applyProviderSettings(
  ctx: MutationCtx,
  authUser: AuthUserReference,
  installationCredential: string,
  activeMacGeneration: number,
  revision: number,
  enabledProviders: Provider[],
): Promise<ProviderSettingsAcknowledgement> {
  assertProviderSettingsRevision(revision);
  const settings = normalizedEnabledProviders(enabledProviders);
  const { device, tokenmaxxer } = await requireActiveDevice(
    ctx,
    authUser,
    installationCredential,
    activeMacGeneration,
  );
  await rateLimiter.limit(ctx, "syncDailyUsage", {
    count: 1,
    key: `${tokenmaxxer._id}:${device._id}:${activeMacGeneration}`,
    throws: true,
  });
  const existing = await ctx.db
    .query("deviceProviderSettings")
    .withIndex("by_device_id", (q) => q.eq("deviceId", device._id))
    .unique();
  if (
    existing &&
    existing.activeMacGeneration === activeMacGeneration &&
    revision < existing.revision
  ) {
    return { outcome: "stale", revision: existing.revision };
  }
  if (
    existing &&
    existing.activeMacGeneration === activeMacGeneration &&
    revision === existing.revision
  ) {
    if (
      existing.claudeEnabled !== settings.claudeEnabled ||
      existing.codexEnabled !== settings.codexEnabled
    ) {
      return { outcome: "stale", revision: existing.revision };
    }
    return { outcome: "idempotent", revision };
  }

  const now = Date.now();
  const values = {
    activeMacGeneration,
    ...settings,
    revision,
    tokenmaxxerId: tokenmaxxer._id,
    updatedAt: now,
  };
  if (existing) {
    if (existing.tokenmaxxerId !== tokenmaxxer._id) {
      throw new Error("provider settings owner is invalid");
    }
    await ctx.db.patch(existing._id, values);
  } else {
    await ctx.db.insert("deviceProviderSettings", {
      ...values,
      deviceId: device._id,
    });
  }
  await ctx.db.patch(device._id, { lastSeenAt: now });
  await ctx.db.patch(tokenmaxxer._id, { lastSyncedAt: now });
  await recomputeScores(ctx, tokenmaxxer._id, rankingDayAt(now));
  return { outcome: "committed", revision };
}
