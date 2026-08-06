import { defineSchema, defineTable } from "convex/server";
import { v } from "convex/values";

const provider = v.union(v.literal("codex"), v.literal("claude"));
const scoreScope = v.union(provider, v.literal("combined"));
const scoreWindow = v.union(v.literal(1), v.literal(7), v.literal(30));
const coverage = v.union(v.literal("complete"), v.literal("partial"));

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
    installationId: v.string(),
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
    apiEquivalentCostMicros: v.optional(v.number()),
    priceBasisVersion: v.optional(v.string()),
    coverage,
    source: v.literal("local-observed"),
    observedAt: v.number(),
    syncedAt: v.number(),
  })
    .index("by_device_id", ["deviceId"])
    .index("by_device_id_and_provider_and_ranking_day", [
      "deviceId",
      "provider",
      "rankingDay",
    ]),

  userDailyUsage: defineTable({
    tokenmaxxerId: v.id("tokenmaxxers"),
    provider,
    rankingDay: v.string(),
    observedTokens: v.number(),
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
