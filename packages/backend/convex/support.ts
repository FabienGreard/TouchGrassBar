import { supportReportSchema } from "@touchgrass/contracts";
import { ConvexError, v } from "convex/values";
import { internal } from "./_generated/api";
import type { Doc } from "./_generated/dataModel";
import { internalMutation, internalQuery, mutation, type MutationCtx } from "./_generated/server";
import { requireAuthUser } from "./auth";
import { profileSessionIsAuthorized, requireActiveDevice } from "./model/profile";
import { rateLimiter } from "./model/rateLimits";
import { supportReportValidator } from "./model/diagnosticValues";
import { supportRequestValidator, supportSessionValidator } from "./model/supportValues";

const SESSION_MS = 30 * 60_000;
const RETENTION_MS = 7 * 24 * 60 * 60_000;
const authorityArgs = { installationCredential: v.string(), activeMacGeneration: v.number() };
const sessionArgs = { ...authorityArgs, sessionId: v.id("supportSessions") };

async function authority(
  ctx: MutationCtx,
  args: { installationCredential: string; activeMacGeneration: number },
) {
  return requireActiveDevice(
    ctx,
    await requireAuthUser(ctx),
    args.installationCredential,
    args.activeMacGeneration,
  );
}
function active(session: Doc<"supportSessions">, now: number) {
  return session.cancelledAt === null && session.expiresAt > now;
}
async function requireSession(
  ctx: MutationCtx,
  args: {
    installationCredential: string;
    activeMacGeneration: number;
    sessionId: Doc<"supportSessions">["_id"];
  },
) {
  const { device, tokenmaxxer } = await authority(ctx, args);
  const session = await ctx.db.get(args.sessionId);
  if (
    !session ||
    session.deviceId !== device._id ||
    session.tokenmaxxerId !== tokenmaxxer._id ||
    session.generation !== device.generation
  )
    throw new ConvexError("SUPPORT_AUTHORITY_REJECTED");
  return session;
}

// Only the person on the active Mac can open this time-limited session.
export const start = mutation({
  args: authorityArgs,
  returns: v.object({ sessionId: v.id("supportSessions"), expiresAt: v.number() }),
  handler: async (ctx, args) => {
    const { device, tokenmaxxer } = await authority(ctx, args);
    const now = Date.now();
    const existing = await ctx.db
      .query("supportSessions")
      .withIndex("by_deviceId", (q) => q.eq("deviceId", device._id))
      .order("desc")
      .first();
    if (existing && active(existing, now) && existing.generation === device.generation)
      return { sessionId: existing._id, expiresAt: existing.expiresAt };
    await rateLimiter.limit(ctx, "supportSessionStart", { key: device._id, throws: true });
    const expiresAt = now + SESSION_MS;
    const sessionId = await ctx.db.insert("supportSessions", {
      tokenmaxxerId: tokenmaxxer._id,
      deviceId: device._id,
      generation: device.generation,
      createdAt: now,
      expiresAt,
      cancelledAt: null,
      deleteAt: expiresAt + RETENTION_MS,
    });
    return { sessionId, expiresAt };
  },
});
export const cancel = mutation({
  args: sessionArgs,
  returns: v.null(),
  handler: async (ctx, args) => {
    const session = await requireSession(ctx, args);
    if (session.cancelledAt === null) await ctx.db.patch(session._id, { cancelledAt: Date.now() });
    return null;
  },
});
export const poll = mutation({
  args: sessionArgs,
  returns: v.object({ active: v.boolean(), requestId: v.union(v.id("supportRequests"), v.null()) }),
  handler: async (ctx, args) => {
    const session = await requireSession(ctx, args);
    if (!active(session, Date.now())) return { active: false, requestId: null };
    const request = await ctx.db
      .query("supportRequests")
      .withIndex("by_sessionId", (q) => q.eq("sessionId", session._id))
      .order("desc")
      .first();
    return {
      active: true,
      requestId:
        request && request.completedAt === null && request.deadline > Date.now()
          ? request._id
          : null,
    };
  },
});
export const complete = mutation({
  args: {
    ...sessionArgs,
    requestId: v.id("supportRequests"),
    report: v.union(supportReportValidator, v.null()),
  },
  returns: v.null(),
  handler: async (ctx, args) => {
    const session = await requireSession(ctx, args);
    const request = await ctx.db.get(args.requestId);
    const now = Date.now();
    if (
      !active(session, now) ||
      !request ||
      request.sessionId !== session._id ||
      request.deadline <= now
    )
      throw new ConvexError("SUPPORT_REQUEST_CLOSED");
    // An acknowledged request is immutable. Network retries are safe.
    if (request.completedAt !== null) return null;
    if (args.report !== null) {
      const parsed = supportReportSchema.safeParse(args.report);
      if (
        !parsed.success ||
        new TextEncoder().encode(JSON.stringify(args.report)).length > 32 * 1024 ||
        args.report.capturedAt < request.requestedAt - 60_000 ||
        args.report.capturedAt > now + 60_000
      )
        throw new ConvexError("SUPPORT_REPORT_INVALID");
    }
    await ctx.db.patch(request._id, {
      completedAt: now,
      report: args.report,
      failure: args.report === null ? "report_unavailable" : null,
    });
    return null;
  },
});

// Deployment administrators can request only this fixed, read-only operation.
export const requestReport = internalMutation({
  args: { touchGrassId: v.string(), operator: v.string() },
  returns: v.id("supportRequests"),
  handler: async (ctx, args) => {
    if (!/^[a-zA-Z0-9@._-]{1,80}$/.test(args.operator))
      throw new ConvexError("SUPPORT_OPERATOR_INVALID");
    const profile = await ctx.db
      .query("tokenmaxxers")
      .withIndex("by_public_id", (q) => q.eq("publicId", args.touchGrassId))
      .unique();
    const session =
      profile &&
      (await ctx.db
        .query("supportSessions")
        .withIndex("by_tokenmaxxerId", (q) => q.eq("tokenmaxxerId", profile._id))
        .order("desc")
        .first());
    const device = session && (await ctx.db.get(session.deviceId));
    const now = Date.now();
    if (
      !profile ||
      !session ||
      !active(session, now) ||
      !device ||
      device.revokedAt !== undefined ||
      profile.activeDeviceId !== device._id ||
      session.generation !== device.generation ||
      profile.authSessionGeneration !== session.generation ||
      !(await profileSessionIsAuthorized(
        ctx,
        profile.authSubject,
        profile.activeAuthSessionId ?? "",
      ))
    )
      throw new ConvexError("SUPPORT_SESSION_UNAVAILABLE");
    const latest = await ctx.db
      .query("supportRequests")
      .withIndex("by_sessionId", (q) => q.eq("sessionId", session._id))
      .order("desc")
      .first();
    if (latest && latest.completedAt === null && latest.deadline > now) return latest._id;
    if (latest && latest.requestedAt > now - 30_000) throw new ConvexError("SUPPORT_RATE_LIMITED");
    return ctx.db.insert("supportRequests", {
      sessionId: session._id,
      requestedAt: now,
      deadline: Math.min(now + 2 * 60_000, session.expiresAt),
      operator: args.operator,
      completedAt: null,
      report: null,
      failure: null,
      deleteAt: now + RETENTION_MS,
    });
  },
});
export const forTouchGrassId = internalQuery({
  args: { touchGrassId: v.string() },
  returns: v.object({
    session: v.union(supportSessionValidator, v.null()),
    requests: v.array(supportRequestValidator),
  }),
  handler: async (ctx, args) => {
    const profile = await ctx.db
      .query("tokenmaxxers")
      .withIndex("by_public_id", (q) => q.eq("publicId", args.touchGrassId))
      .unique();
    const session =
      profile &&
      (await ctx.db
        .query("supportSessions")
        .withIndex("by_tokenmaxxerId", (q) => q.eq("tokenmaxxerId", profile._id))
        .order("desc")
        .first());
    if (!session) return { session: null, requests: [] };
    return {
      session,
      requests: await ctx.db
        .query("supportRequests")
        .withIndex("by_sessionId", (q) => q.eq("sessionId", session._id))
        .order("desc")
        .take(10),
    };
  },
});
export const deleteExpired = internalMutation({
  args: {},
  returns: v.null(),
  handler: async (ctx) => {
    let more = false;
    for (const table of ["supportRequests", "supportSessions"] as const) {
      const expired = await ctx.db
        .query(table)
        .withIndex("by_deleteAt", (q) => q.lte("deleteAt", Date.now()))
        .take(50);
      for (const row of expired) await ctx.db.delete(row._id);
      more ||= expired.length === 50;
    }
    if (more) await ctx.scheduler.runAfter(0, internal.support.deleteExpired, {});
    return null;
  },
});
