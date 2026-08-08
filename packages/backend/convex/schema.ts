import { defineSchema, defineTable } from "convex/server";
import { v } from "convex/values";

import {
  apiEquivalentCostValidator as apiEquivalentCost,
  apiEquivalentCostValueValidator as apiEquivalentCostValue,
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
    // Optional legacy field. New Active Macs store only a digest of the
    // Keychain-held installation credential.
    installationId: v.optional(v.string()),
    installationCredentialDigest: v.optional(v.string()),
    generation: v.optional(v.number()),
    createdAt: v.number(),
    lastSeenAt: v.number(),
    revokedAt: v.optional(v.number()),
  })
    .index("by_tokenmaxxer_id", ["tokenmaxxerId"])
    .index("by_tokenmaxxer_id_and_installation_id", ["tokenmaxxerId", "installationId"]),

  usageBuckets: defineTable({
    deviceId: v.id("devices"),
    tokenmaxxerId: v.id("tokenmaxxers"),
    provider,
    rankingDay: v.string(),
    revision: v.number(),
    observedTokens: v.number(),
    // Optional legacy fields. The complete cost object below is the current
    // write contract.
    apiEquivalentCostMicros: v.optional(v.number()),
    priceBasisVersion: v.optional(v.string()),
    apiEquivalentCost: v.optional(apiEquivalentCost),
    coverage,
    source: v.optional(v.literal("local-observed")),
    evidenceBasis: v.optional(evidenceBasis),
    correctionReason: v.optional(v.union(correctionReason, v.null())),
    correctionRevision: v.optional(v.union(v.number(), v.null())),
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

  userDailyUsage: defineTable({
    tokenmaxxerId: v.id("tokenmaxxers"),
    provider,
    rankingDay: v.string(),
    observedTokens: v.number(),
    apiEquivalentCost: v.optional(apiEquivalentCostValue),
    // Compatibility fields for rows written before the complete cost object.
    apiEquivalentCostMicros: v.optional(v.number()),
    costIsComplete: v.boolean(),
    updatedAt: v.number(),
  })
    .index("by_tokenmaxxer_id", ["tokenmaxxerId"])
    .index("by_tokenmaxxer_id_and_provider_and_ranking_day", [
      "tokenmaxxerId",
      "provider",
      "rankingDay",
    ]),

  userScores: defineTable({
    tokenmaxxerId: v.id("tokenmaxxers"),
    boardKey: v.string(),
    scope: scoreScope,
    windowDays: scoreWindow,
    tokenScore: v.number(),
    apiEquivalentCost: v.optional(apiEquivalentCostValue),
    // Compatibility field for clients that read only the numeric estimate.
    apiEquivalentCostMicros: v.optional(v.number()),
    computedAt: v.number(),
  })
    .index("by_tokenmaxxer_id", ["tokenmaxxerId"])
    .index("by_board_key", ["boardKey"])
    .index("by_tokenmaxxer_id_and_scope_and_window_days", [
      "tokenmaxxerId",
      "scope",
      "windowDays",
    ]),

  publicScores: defineTable({
    tokenmaxxerId: v.id("tokenmaxxers"),
    boardKey: v.string(),
    scope: scoreScope,
    windowDays: scoreWindow,
    tokenScore: v.number(),
    apiEquivalentCost: v.optional(apiEquivalentCostValue),
    // Compatibility field for clients that read only the numeric estimate.
    apiEquivalentCostMicros: v.optional(v.number()),
    displayName: v.string(),
    touchGrassId: v.string(),
    computedAt: v.number(),
  })
    .index("by_tokenmaxxer_id", ["tokenmaxxerId"])
    .index("by_board_key", ["boardKey"])
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
