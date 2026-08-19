import { defineSchema, defineTable } from "convex/server";
import { v } from "convex/values";

import {
  apiEquivalentCostValidator as apiEquivalentCost,
  correctionReasonValidator as correctionReason,
  coverageValidator as coverage,
  evidenceBasisValidator as evidenceBasis,
  providerValidator as provider,
  scoreScopeValidator as scoreScope,
  scoreWindowValidator as scoreWindow,
} from "./model/values";

export default defineSchema({
  signupProofs: defineTable({
    expiresAt: v.number(),
    nonceDigest: v.string(),
    touchGrassId: v.string(),
  }).index("by_nonce_digest", ["nonceDigest"]),

  recoveryKeyAttemptReservations: defineTable({
    expiresAt: v.number(),
    ipKey: v.string(),
    touchGrassIdKey: v.string(),
  })
    .index("by_ip_key_and_expires_at", ["ipKey", "expiresAt"])
    .index("by_touch_grass_id_key_and_expires_at", [
      "touchGrassIdKey",
      "expiresAt",
    ]),

  tokenmaxxers: defineTable({
    activeDeviceId: v.optional(v.id("devices")),
    authSubject: v.string(),
    displayName: v.string(),
    publicId: v.string(),
    createdAt: v.number(),
    lastSyncedAt: v.optional(v.number()),
  })
    .index("by_auth_subject", ["authSubject"])
    .index("by_public_id", ["publicId"])
    .index("by_last_synced_at", ["lastSyncedAt"]),

  devices: defineTable({
    tokenmaxxerId: v.id("tokenmaxxers"),
    installationCredentialDigest: v.string(),
    generation: v.number(),
    createdAt: v.number(),
    lastSeenAt: v.number(),
    revokedAt: v.optional(v.number()),
    usageBackfillCompletedAt: v.optional(v.union(v.number(), v.null())),
  }).index("by_tokenmaxxer_id", ["tokenmaxxerId"]),

  usageBuckets: defineTable({
    deviceId: v.id("devices"),
    tokenmaxxerId: v.id("tokenmaxxers"),
    provider,
    rankingDay: v.string(),
    revision: v.number(),
    observedTokens: v.number(),
    apiEquivalentCost,
    coverage,
    evidenceBasis,
    correctionReason: v.union(correctionReason, v.null()),
    correctionRevision: v.union(v.number(), v.null()),
    lastCorrectionReason: v.optional(correctionReason),
    lastCorrectionRevision: v.optional(v.number()),
    observedAt: v.number(),
    syncedAt: v.number(),
  })
    .index("by_device_id", ["deviceId"])
    .index("by_device_id_and_provider_and_ranking_day", [
      "deviceId",
      "provider",
      "rankingDay",
    ])
    .index("by_tokenmaxxer_id_and_provider_and_ranking_day", [
      "tokenmaxxerId",
      "provider",
      "rankingDay",
    ]),

  usageCorrectionAudits: defineTable({
    bucketId: v.id("usageBuckets"),
    deviceId: v.id("devices"),
    tokenmaxxerId: v.id("tokenmaxxers"),
    provider,
    rankingDay: v.string(),
    revision: v.number(),
    reason: correctionReason,
    createdAt: v.number(),
  }).index("by_bucket_id_and_revision", ["bucketId", "revision"]),

  deviceProviderSettings: defineTable({
    activeMacGeneration: v.number(),
    claudeEnabled: v.boolean(),
    codexEnabled: v.boolean(),
    deviceId: v.id("devices"),
    revision: v.number(),
    tokenmaxxerId: v.id("tokenmaxxers"),
    updatedAt: v.number(),
  }).index("by_device_id", ["deviceId"]),

  userDailyUsage: defineTable({
    tokenmaxxerId: v.id("tokenmaxxers"),
    provider,
    rankingDay: v.string(),
    observedTokens: v.number(),
    apiEquivalentCost,
    updatedAt: v.number(),
  })
    .index("by_tokenmaxxer_id", ["tokenmaxxerId"])
    .index("by_tokenmaxxer_id_and_provider_and_ranking_day", [
      "tokenmaxxerId",
      "provider",
      "rankingDay",
    ]),

  publicUsages: defineTable({
    tokenmaxxerId: v.id("tokenmaxxers"),
    boardKey: v.string(),
    scope: scoreScope,
    windowDays: scoreWindow,
    tokenScore: v.number(),
    apiEquivalentCost,
    displayName: v.string(),
    touchGrassId: v.string(),
    computedAt: v.number(),
  })
    .index("by_tokenmaxxer_id", ["tokenmaxxerId"])
    .index("by_board_key", ["boardKey"])
    .index("by_board_key_and_token_score_and_touch_grass_id", [
      "boardKey",
      "tokenScore",
      "touchGrassId",
    ])
    .index("by_tokenmaxxer_id_and_scope_and_window_days", [
      "tokenmaxxerId",
      "scope",
      "windowDays",
    ]),

  addedTokenmaxxers: defineTable({
    ownerId: v.id("tokenmaxxers"),
    addedId: v.id("tokenmaxxers"),
    createdAt: v.number(),
  })
    .index("by_owner_id", ["ownerId"])
    .index("by_owner_id_and_added_id", ["ownerId", "addedId"]),
});
