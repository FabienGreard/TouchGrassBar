import { v } from "convex/values";

import { internal } from "../_generated/api";
import { internalMutation } from "../_generated/server";
import { rateLimiter, touchGrassAuthPolicy } from "../model/rateLimits";

export const reserveCredentialAttempt = internalMutation({
  args: {
    ipKey: v.string(),
    touchGrassIdKey: v.string(),
  },
  returns: v.union(v.id("recoveryKeyAttemptReservations"), v.null()),
  handler: async (ctx, args) => {
    const now = Date.now();
    const attempts = touchGrassAuthPolicy.failedRecoveryKey.attempts;
    const [ipReservations, touchGrassIdReservations] = await Promise.all([
      ctx.db
        .query("recoveryKeyAttemptReservations")
        .withIndex("by_ip_key_and_expires_at", (query) =>
          query.eq("ipKey", args.ipKey).gt("expiresAt", now),
        )
        .take(attempts),
      ctx.db
        .query("recoveryKeyAttemptReservations")
        .withIndex("by_touch_grass_id_key_and_expires_at", (query) =>
          query.eq("touchGrassIdKey", args.touchGrassIdKey).gt("expiresAt", now),
        )
        .take(attempts),
    ]);
    if (ipReservations.length >= attempts || touchGrassIdReservations.length >= attempts) {
      return null;
    }

    const [ipLimit, touchGrassIdLimit] = await Promise.all([
      rateLimiter.check(ctx, "failedRecoveryKeyByIp", {
        count: ipReservations.length + 1,
        key: args.ipKey,
      }),
      rateLimiter.check(ctx, "failedRecoveryKeyByTouchGrassId", {
        count: touchGrassIdReservations.length + 1,
        key: args.touchGrassIdKey,
      }),
    ]);
    if (!ipLimit.ok || !touchGrassIdLimit.ok) return null;

    const expiresAt = now + touchGrassAuthPolicy.failedRecoveryKey.reservationMs;
    const reservationId = await ctx.db.insert("recoveryKeyAttemptReservations", {
      ...args,
      expiresAt,
    });
    await ctx.scheduler.runAt(
      expiresAt,
      internal.auth.credentialAttempts.expireCredentialAttemptReservation,
      { reservationId },
    );
    return reservationId;
  },
});

export const finalizeCredentialAttempt = internalMutation({
  args: {
    outcome: v.union(v.literal("failure"), v.literal("success")),
    reservationId: v.id("recoveryKeyAttemptReservations"),
  },
  returns: v.boolean(),
  handler: async (ctx, args) => {
    const reservation = await ctx.db.get(args.reservationId);
    if (!reservation) return false;

    if (args.outcome === "failure") {
      await rateLimiter.limit(ctx, "failedRecoveryKeyByIp", {
        key: reservation.ipKey,
        throws: true,
      });
      await rateLimiter.limit(ctx, "failedRecoveryKeyByTouchGrassId", {
        key: reservation.touchGrassIdKey,
        throws: true,
      });
    }
    await ctx.db.delete(reservation._id);
    return true;
  },
});

export const expireCredentialAttemptReservation = internalMutation({
  args: { reservationId: v.id("recoveryKeyAttemptReservations") },
  returns: v.null(),
  handler: async (ctx, args): Promise<null> => {
    const reservation = await ctx.db.get(args.reservationId);
    if (!reservation) return null;
    if (reservation.expiresAt > Date.now()) {
      await ctx.scheduler.runAt(
        reservation.expiresAt,
        internal.auth.credentialAttempts.expireCredentialAttemptReservation,
        args,
      );
      return null;
    }
    await ctx.db.delete(reservation._id);
    return null;
  },
});
