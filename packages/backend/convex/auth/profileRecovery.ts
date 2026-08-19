import { v } from "convex/values";

import { internal } from "../_generated/api";
import { internalMutation, type MutationCtx } from "../_generated/server";
import { installationCredentialDigest } from "../model/profile";
import { rateLimiter } from "../model/rateLimits";

const RECOVERY_ATTEMPT_LIFETIME_MS = 5 * 60 * 1_000;
const MAX_SAFE_GENERATION = Number.MAX_SAFE_INTEGER;

const recoveryCommitResult = v.object({
  activeMacActivatedAt: v.number(),
  activeMacGeneration: v.number(),
  displayName: v.string(),
  touchGrassId: v.string(),
});

async function recoveryAttemptByDigest(
  ctx: MutationCtx,
  attemptDigest: string,
) {
  return ctx.db
    .query("profileRecoveryAttempts")
    .withIndex("by_attempt_digest", (query) =>
      query.eq("attemptDigest", attemptDigest),
    )
    .unique();
}

export const prepareRecoveryAttempt = internalMutation({
  args: {
    attemptDigest: v.string(),
    authSubject: v.string(),
    touchGrassId: v.string(),
  },
  returns: v.union(
    v.object({ expectedGeneration: v.number(), expiresAt: v.number() }),
    v.null(),
  ),
  handler: async (ctx, args) => {
    const tokenmaxxer = await ctx.db
      .query("tokenmaxxers")
      .withIndex("by_auth_subject", (query) =>
        query.eq("authSubject", args.authSubject),
      )
      .unique();
    if (
      !tokenmaxxer?.activeDeviceId ||
      tokenmaxxer.publicId !== args.touchGrassId
    ) {
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
    if (existing) {
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
    await ctx.scheduler.runAt(
      expiresAt,
      internal.auth.profileRecovery.expireRecoveryAttempt,
      { recoveryAttemptId },
    );
    return { expectedGeneration: activeDevice.generation, expiresAt };
  },
});

export const claimRecoveryAttempt = internalMutation({
  args: { attemptDigest: v.string(), authSubject: v.string() },
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
      return tokenmaxxer.recoveryAttemptId === attempt._id;
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
      if (tokenmaxxer.recoveryAttemptId === attempt._id) return true;
      const previousAttempt = await ctx.db.get(tokenmaxxer.recoveryAttemptId);
      if (
        !previousAttempt ||
        previousAttempt.status !== "committed" ||
        previousAttempt.expectedGeneration >= attempt.expectedGeneration
      ) {
        return false;
      }
    }
    await rateLimiter.limit(ctx, "successfulProfileRecovery", {
      key: String(tokenmaxxer._id),
      throws: true,
    });
    await ctx.db.patch(attempt._id, { status: "committing" });
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
      tokenmaxxer.recoveryAttemptId !== attempt._id
    ) {
      return null;
    }
    if (attempt.status === "committed") {
      if (
        attempt.activatedAt === undefined ||
        attempt.newDeviceId === undefined
      ) {
        return null;
      }
      return {
        activeMacActivatedAt: attempt.activatedAt,
        activeMacGeneration: attempt.expectedGeneration + 1,
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
      installationCredentialDigest: await installationCredentialDigest(
        args.installationCredential,
      ),
      lastSeenAt: activatedAt,
      tokenmaxxerId: tokenmaxxer._id,
      usageBackfillCompletedAt: null,
    });
    await ctx.db.patch(oldDevice._id, { revokedAt: activatedAt });
    await ctx.db.patch(tokenmaxxer._id, { activeDeviceId: newDeviceId });
    await ctx.db.patch(attempt._id, {
      activatedAt,
      committedAt: activatedAt,
      newDeviceId,
      status: "committed",
    });
    return {
      activeMacActivatedAt: activatedAt,
      activeMacGeneration: generation,
      displayName: tokenmaxxer.displayName,
      touchGrassId: tokenmaxxer.publicId,
    };
  },
});

export const expireRecoveryAttempt = internalMutation({
  args: { recoveryAttemptId: v.id("profileRecoveryAttempts") },
  returns: v.null(),
  handler: async (ctx, args): Promise<null> => {
    const attempt = await ctx.db.get(args.recoveryAttemptId);
    if (!attempt || attempt.status !== "prepared") return null;
    if (attempt.expiresAt > Date.now()) {
      await ctx.scheduler.runAt(
        attempt.expiresAt,
        internal.auth.profileRecovery.expireRecoveryAttempt,
        args,
      );
      return null;
    }
    await ctx.db.delete(attempt._id);
    return null;
  },
});
