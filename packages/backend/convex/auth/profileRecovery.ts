import { v } from "convex/values";

import { internal } from "../_generated/api";
import { internalMutation, internalQuery, type MutationCtx } from "../_generated/server";
import { installationCredentialDigest, profileSessionIsAuthorized } from "../model/profile";
import { rateLimiter } from "../model/rateLimits";
import { freezeTransferDayUsage } from "../model/sync";
import { rankingDayAt } from "../model/values";

const RECOVERY_ATTEMPT_LIFETIME_MS = 5 * 60 * 1_000;
const RECOVERY_COMMIT_GRACE_MS = 60 * 1_000;
const RECOVERY_AUTH_FINALIZATION_LEASE_MS = 60 * 1_000;
const MAX_SAFE_GENERATION = Number.MAX_SAFE_INTEGER;

const recoveryCommitResult = v.object({
  activeMacActivatedAt: v.number(),
  activeMacGeneration: v.number(),
  authFinalized: v.boolean(),
  displayName: v.string(),
  touchGrassId: v.string(),
});

async function recoveryAttemptByDigest(ctx: MutationCtx, attemptDigest: string) {
  return ctx.db
    .query("profileRecoveryAttempts")
    .withIndex("by_attempt_digest", (query) => query.eq("attemptDigest", attemptDigest))
    .unique();
}

export const prepareRecoveryAttempt = internalMutation({
  args: {
    attemptDigest: v.string(),
    authSubject: v.string(),
    touchGrassId: v.string(),
  },
  returns: v.union(v.object({ expectedGeneration: v.number(), expiresAt: v.number() }), v.null()),
  handler: async (ctx, args) => {
    const tokenmaxxer = await ctx.db
      .query("tokenmaxxers")
      .withIndex("by_auth_subject", (query) => query.eq("authSubject", args.authSubject))
      .unique();
    if (!tokenmaxxer?.activeDeviceId || tokenmaxxer.publicId !== args.touchGrassId) {
      return null;
    }
    const activeDevice = await ctx.db.get(tokenmaxxer.activeDeviceId);
    if (
      !activeDevice ||
      activeDevice.tokenmaxxerId !== tokenmaxxer._id ||
      activeDevice.revokedAt !== undefined
    ) {
      return null;
    }
    const existing = await recoveryAttemptByDigest(ctx, args.attemptDigest);
    if (tokenmaxxer.recoveryAttemptId !== undefined) {
      const activeAttempt = await ctx.db.get(tokenmaxxer.recoveryAttemptId);
      if (
        activeAttempt?.status === "committed" &&
        activeAttempt.authFinalizedAt === undefined &&
        existing?._id !== activeAttempt._id
      ) {
        return null;
      }
    }
    if (existing) {
      if (existing.status === "committed" && tokenmaxxer.recoveryAttemptId === existing._id) {
        const expiresAt = Date.now() + RECOVERY_ATTEMPT_LIFETIME_MS;
        await ctx.db.patch(existing._id, { expiresAt });
        await ctx.scheduler.runAt(expiresAt, internal.auth.profileRecovery.expireRecoveryAttempt, {
          recoveryAttemptId: existing._id,
        });
        return {
          expectedGeneration: existing.expectedGeneration,
          expiresAt,
        };
      }
      if (
        ((existing.status === "prepared" && existing.expiresAt <= Date.now()) ||
          (existing.status === "committing" && tokenmaxxer.recoveryAttemptId === existing._id)) &&
        existing.expectedDeviceId === activeDevice._id &&
        existing.expectedGeneration === activeDevice.generation
      ) {
        const expiresAt = Date.now() + RECOVERY_ATTEMPT_LIFETIME_MS;
        await ctx.db.patch(existing._id, { expiresAt });
        await ctx.scheduler.runAt(expiresAt, internal.auth.profileRecovery.expireRecoveryAttempt, {
          recoveryAttemptId: existing._id,
        });
        return { expectedGeneration: existing.expectedGeneration, expiresAt };
      }
      if (
        existing.tokenmaxxerId !== tokenmaxxer._id ||
        existing.expectedDeviceId !== activeDevice._id ||
        existing.expectedGeneration !== activeDevice.generation ||
        existing.status !== "prepared" ||
        existing.expiresAt <= Date.now()
      ) {
        return null;
      }
      return {
        expectedGeneration: existing.expectedGeneration,
        expiresAt: existing.expiresAt,
      };
    }

    const expiresAt = Date.now() + RECOVERY_ATTEMPT_LIFETIME_MS;
    const recoveryAttemptId = await ctx.db.insert("profileRecoveryAttempts", {
      attemptDigest: args.attemptDigest,
      expectedDeviceId: activeDevice._id,
      expectedGeneration: activeDevice.generation,
      expiresAt,
      status: "prepared",
      tokenmaxxerId: tokenmaxxer._id,
    });
    await ctx.scheduler.runAt(expiresAt, internal.auth.profileRecovery.expireRecoveryAttempt, {
      recoveryAttemptId,
    });
    return { expectedGeneration: activeDevice.generation, expiresAt };
  },
});

export const claimRecoveryAttempt = internalMutation({
  args: {
    attemptDigest: v.string(),
    authSubject: v.string(),
    installationCredentialDigest: v.string(),
    replacementRecoveryKeyDigest: v.string(),
  },
  returns: v.boolean(),
  handler: async (ctx, args) => {
    const attempt = await recoveryAttemptByDigest(ctx, args.attemptDigest);
    if (!attempt) return false;
    const tokenmaxxer = await ctx.db.get(attempt.tokenmaxxerId);
    if (
      !tokenmaxxer ||
      tokenmaxxer.authSubject !== args.authSubject ||
      !tokenmaxxer.activeDeviceId
    ) {
      return false;
    }
    if (attempt.status === "committed") {
      return (
        tokenmaxxer.recoveryAttemptId === attempt._id &&
        attempt.installationCredentialDigest === args.installationCredentialDigest &&
        attempt.replacementRecoveryKeyDigest === args.replacementRecoveryKeyDigest
      );
    }
    if (
      attempt.expiresAt <= Date.now() ||
      tokenmaxxer.activeDeviceId !== attempt.expectedDeviceId
    ) {
      return false;
    }
    const activeDevice = await ctx.db.get(tokenmaxxer.activeDeviceId);
    if (
      !activeDevice ||
      activeDevice.revokedAt !== undefined ||
      activeDevice.generation !== attempt.expectedGeneration
    ) {
      return false;
    }
    if (tokenmaxxer.recoveryAttemptId) {
      if (tokenmaxxer.recoveryAttemptId === attempt._id) {
        return (
          attempt.installationCredentialDigest === args.installationCredentialDigest &&
          attempt.replacementRecoveryKeyDigest === args.replacementRecoveryKeyDigest
        );
      }
      const previousAttempt = await ctx.db.get(tokenmaxxer.recoveryAttemptId);
      if (
        !previousAttempt ||
        previousAttempt.status !== "committed" ||
        previousAttempt.authFinalizedAt === undefined ||
        previousAttempt.expectedGeneration >= attempt.expectedGeneration
      ) {
        return false;
      }
    }
    await rateLimiter.limit(ctx, "successfulProfileRecovery", {
      key: String(tokenmaxxer._id),
      throws: true,
    });
    await ctx.db.patch(attempt._id, {
      installationCredentialDigest: args.installationCredentialDigest,
      replacementRecoveryKeyDigest: args.replacementRecoveryKeyDigest,
      status: "committing",
    });
    await ctx.db.patch(tokenmaxxer._id, { recoveryAttemptId: attempt._id });
    return true;
  },
});

export const commitRecoveryAttempt = internalMutation({
  args: {
    attemptDigest: v.string(),
    authSubject: v.string(),
    installationCredential: v.string(),
  },
  returns: v.union(recoveryCommitResult, v.null()),
  handler: async (ctx, args) => {
    const attempt = await recoveryAttemptByDigest(ctx, args.attemptDigest);
    if (!attempt) return null;
    const tokenmaxxer = await ctx.db.get(attempt.tokenmaxxerId);
    if (
      !tokenmaxxer ||
      tokenmaxxer.authSubject !== args.authSubject ||
      tokenmaxxer.recoveryAttemptId !== attempt._id ||
      attempt.installationCredentialDigest !==
        (await installationCredentialDigest(args.installationCredential))
    ) {
      return null;
    }
    if (attempt.status === "committed") {
      if (attempt.activatedAt === undefined || attempt.newDeviceId === undefined) {
        return null;
      }
      return {
        activeMacActivatedAt: attempt.activatedAt,
        activeMacGeneration: attempt.expectedGeneration + 1,
        authFinalized: attempt.authFinalizedAt !== undefined,
        displayName: tokenmaxxer.displayName,
        touchGrassId: tokenmaxxer.publicId,
      };
    }
    if (
      attempt.status !== "committing" ||
      tokenmaxxer.activeDeviceId !== attempt.expectedDeviceId ||
      attempt.expectedGeneration >= MAX_SAFE_GENERATION
    ) {
      return null;
    }
    const oldDevice = await ctx.db.get(attempt.expectedDeviceId);
    if (
      !oldDevice ||
      oldDevice.revokedAt !== undefined ||
      oldDevice.generation !== attempt.expectedGeneration
    ) {
      return null;
    }
    const activatedAt = Date.now();
    const generation = attempt.expectedGeneration + 1;
    const newDeviceId = await ctx.db.insert("devices", {
      createdAt: activatedAt,
      generation,
      installationCredentialDigest: await installationCredentialDigest(args.installationCredential),
      lastSeenAt: activatedAt,
      tokenmaxxerId: tokenmaxxer._id,
      usageBackfillCompletedAt: null,
    });
    await ctx.db.patch(oldDevice._id, { revokedAt: activatedAt });
    const transferDay = rankingDayAt(activatedAt);
    await freezeTransferDayUsage(ctx, tokenmaxxer._id, oldDevice._id, newDeviceId, transferDay);
    await ctx.db.patch(tokenmaxxer._id, {
      activeAuthSessionId: undefined,
      activeDeviceId: newDeviceId,
      authSessionGeneration: generation,
    });
    await ctx.db.patch(attempt._id, {
      activatedAt,
      committedAt: activatedAt,
      newDeviceId,
      status: "committed",
    });
    return {
      activeMacActivatedAt: activatedAt,
      activeMacGeneration: generation,
      authFinalized: false,
      displayName: tokenmaxxer.displayName,
      touchGrassId: tokenmaxxer.publicId,
    };
  },
});

export const claimRecoveryAuthFinalization = internalMutation({
  args: {
    attemptDigest: v.string(),
    authSubject: v.string(),
    claim: v.string(),
  },
  returns: v.boolean(),
  handler: async (ctx, args) => {
    const attempt = await recoveryAttemptByDigest(ctx, args.attemptDigest);
    if (!attempt || attempt.status !== "committed") return false;
    const tokenmaxxer = await ctx.db.get(attempt.tokenmaxxerId);
    if (
      !tokenmaxxer ||
      tokenmaxxer.authSubject !== args.authSubject ||
      tokenmaxxer.recoveryAttemptId !== attempt._id ||
      attempt.authFinalizedAt !== undefined ||
      (attempt.authFinalizationLeaseExpiresAt !== undefined &&
        attempt.authFinalizationLeaseExpiresAt > Date.now() &&
        attempt.authFinalizationClaim !== args.claim)
    ) {
      return false;
    }
    await ctx.db.patch(attempt._id, {
      authFinalizationClaim: args.claim,
      authFinalizationLeaseExpiresAt: Date.now() + RECOVERY_AUTH_FINALIZATION_LEASE_MS,
    });
    return true;
  },
});

export const finalizeRecoveryAuth = internalMutation({
  args: {
    attemptDigest: v.string(),
    authSubject: v.string(),
    claim: v.string(),
  },
  returns: v.boolean(),
  handler: async (ctx, args) => {
    const attempt = await recoveryAttemptByDigest(ctx, args.attemptDigest);
    if (!attempt || attempt.status !== "committed") return false;
    const tokenmaxxer = await ctx.db.get(attempt.tokenmaxxerId);
    if (
      !tokenmaxxer ||
      tokenmaxxer.authSubject !== args.authSubject ||
      tokenmaxxer.recoveryAttemptId !== attempt._id ||
      attempt.authFinalizationClaim !== args.claim
    ) {
      return false;
    }
    if (attempt.authFinalizedAt === undefined) {
      await ctx.db.patch(attempt._id, {
        authFinalizationClaim: undefined,
        authFinalizationLeaseExpiresAt: undefined,
        authFinalizedAt: Date.now(),
      });
    }
    return true;
  },
});

export const releaseRecoveryAuthFinalization = internalMutation({
  args: {
    attemptDigest: v.string(),
    authSubject: v.string(),
    claim: v.string(),
  },
  returns: v.null(),
  handler: async (ctx, args) => {
    const attempt = await recoveryAttemptByDigest(ctx, args.attemptDigest);
    if (!attempt || attempt.status !== "committed") return null;
    const tokenmaxxer = await ctx.db.get(attempt.tokenmaxxerId);
    if (
      tokenmaxxer?.authSubject === args.authSubject &&
      tokenmaxxer.recoveryAttemptId === attempt._id &&
      attempt.authFinalizedAt === undefined &&
      attempt.authFinalizationClaim === args.claim
    ) {
      await ctx.db.patch(attempt._id, {
        authFinalizationClaim: undefined,
        authFinalizationLeaseExpiresAt: undefined,
      });
    }
    return null;
  },
});

export const recoveryAuthPending = internalQuery({
  args: { touchGrassId: v.string() },
  returns: v.boolean(),
  handler: async (ctx, args) => {
    const tokenmaxxer = await ctx.db
      .query("tokenmaxxers")
      .withIndex("by_public_id", (query) => query.eq("publicId", args.touchGrassId))
      .unique();
    if (!tokenmaxxer?.recoveryAttemptId) return false;
    const attempt = await ctx.db.get(tokenmaxxer.recoveryAttemptId);
    return attempt?.status === "committed" && attempt.authFinalizedAt === undefined;
  },
});

export const authorizeProfileSession = internalMutation({
  args: {
    activeMacGeneration: v.number(),
    authSubject: v.string(),
    sessionId: v.string(),
    touchGrassId: v.string(),
  },
  returns: v.boolean(),
  handler: async (ctx, args) => {
    const tokenmaxxer = await ctx.db
      .query("tokenmaxxers")
      .withIndex("by_public_id", (query) => query.eq("publicId", args.touchGrassId))
      .unique();
    if (!tokenmaxxer?.activeDeviceId || tokenmaxxer.authSubject !== args.authSubject) {
      return false;
    }
    if (tokenmaxxer.recoveryAttemptId) {
      const attempt = await ctx.db.get(tokenmaxxer.recoveryAttemptId);
      if (attempt?.status === "committed" && attempt.authFinalizedAt === undefined) {
        return false;
      }
    }
    const activeDevice = await ctx.db.get(tokenmaxxer.activeDeviceId);
    if (
      activeDevice?.tokenmaxxerId !== tokenmaxxer._id ||
      activeDevice.revokedAt !== undefined ||
      activeDevice.generation !== args.activeMacGeneration
    ) {
      return false;
    }
    await ctx.db.patch(tokenmaxxer._id, {
      activeAuthSessionId: args.sessionId,
      authSessionGeneration: args.activeMacGeneration,
    });
    return true;
  },
});

export const profileSessionAuthorized = internalQuery({
  args: { authSubject: v.string(), sessionId: v.string() },
  returns: v.boolean(),
  handler: (ctx, args) => profileSessionIsAuthorized(ctx, args.authSubject, args.sessionId),
});

export const profileAuthGeneration = internalQuery({
  args: { touchGrassId: v.string() },
  returns: v.union(v.number(), v.null()),
  handler: async (ctx, args) => {
    const tokenmaxxer = await ctx.db
      .query("tokenmaxxers")
      .withIndex("by_public_id", (query) => query.eq("publicId", args.touchGrassId))
      .unique();
    if (!tokenmaxxer?.activeDeviceId) return null;
    const device = await ctx.db.get(tokenmaxxer.activeDeviceId);
    return device?.tokenmaxxerId === tokenmaxxer._id && device.revokedAt === undefined
      ? device.generation
      : null;
  },
});

export const expireRecoveryAttempt = internalMutation({
  args: { recoveryAttemptId: v.id("profileRecoveryAttempts") },
  returns: v.null(),
  handler: async (ctx, args): Promise<null> => {
    const attempt = await ctx.db.get(args.recoveryAttemptId);
    if (!attempt || attempt.status === "committed") return null;
    const removeAt =
      attempt.status === "committing"
        ? attempt.expiresAt + RECOVERY_COMMIT_GRACE_MS
        : attempt.expiresAt;
    if (removeAt > Date.now()) {
      await ctx.scheduler.runAt(
        removeAt,
        internal.auth.profileRecovery.expireRecoveryAttempt,
        args,
      );
      return null;
    }
    if (attempt.status === "committing") {
      const tokenmaxxer = await ctx.db.get(attempt.tokenmaxxerId);
      if (tokenmaxxer?.recoveryAttemptId === attempt._id) {
        await ctx.db.patch(tokenmaxxer._id, {
          recoveryAttemptId: undefined,
        });
      }
    }
    await ctx.db.delete(attempt._id);
    return null;
  },
});
