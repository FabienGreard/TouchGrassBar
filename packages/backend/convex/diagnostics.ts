import { DIAGNOSTIC_SERVER_RETENTION_MS } from "@touchgrass/contracts";
import { paginationOptsValidator, paginationResultValidator } from "convex/server";
import { ConvexError, v } from "convex/values";

import { internal } from "./_generated/api";
import { internalMutation, internalQuery, mutation } from "./_generated/server";
import { requireAuthUser } from "./auth";
import {
  diagnosticReportValidator,
  diagnosticStoredReportValidator,
} from "./model/diagnosticValues";
import {
  assertDiagnosticReportAge,
  diagnosticCredentialDigest,
  diagnosticGroupKey,
  diagnosticPayloadDigest,
  parseDiagnosticReport,
  requireDiagnosticReporter,
} from "./model/diagnostics";
import { requireActiveDevice } from "./model/profile";
import { rateLimiter } from "./model/rateLimits";
import { providerValidator } from "./model/values";

export const register = mutation({
  args: {
    installationCredential: v.string(),
    activeMacGeneration: v.number(),
    diagnosticCredential: v.string(),
  },
  returns: v.object({ reporterId: v.id("diagnosticReporters"), generation: v.number() }),
  handler: async (ctx, args) => {
    const user = await requireAuthUser(ctx);
    const { device, tokenmaxxer } = await requireActiveDevice(
      ctx,
      user,
      args.installationCredential,
      args.activeMacGeneration,
    );
    const credentialDigest = await diagnosticCredentialDigest(args.diagnosticCredential);
    const existing = await ctx.db
      .query("diagnosticReporters")
      .withIndex("by_deviceId", (q) => q.eq("deviceId", device._id))
      .unique();
    if (existing) {
      if (
        existing.revokedAt !== null ||
        existing.generation !== device.generation ||
        existing.tokenmaxxerId !== tokenmaxxer._id
      ) {
        throw new ConvexError("DIAGNOSTIC_AUTHORITY_REJECTED");
      }
      if (existing.credentialDigest !== credentialDigest) {
        await rateLimiter.limit(ctx, "diagnosticRegistration", { key: device._id, throws: true });
        await ctx.db.patch(existing._id, { credentialDigest });
      }
      return { reporterId: existing._id, generation: existing.generation };
    }
    await rateLimiter.limit(ctx, "diagnosticRegistration", { key: device._id, throws: true });
    const reporterId = await ctx.db.insert("diagnosticReporters", {
      tokenmaxxerId: tokenmaxxer._id,
      deviceId: device._id,
      generation: device.generation,
      credentialDigest,
      createdAt: Date.now(),
      revokedAt: null,
    });
    return { reporterId, generation: device.generation };
  },
});

export const submit = mutation({
  args: {
    reporterId: v.id("diagnosticReporters"),
    diagnosticCredential: v.string(),
    report: diagnosticReportValidator,
  },
  returns: v.object({
    outcome: v.union(v.literal("accepted"), v.literal("duplicate"), v.literal("rate_limited")),
    retryAfterMs: v.union(v.number(), v.null()),
  }),
  handler: async (ctx, args) => {
    const reporter = await requireDiagnosticReporter(
      ctx,
      args.reporterId,
      args.diagnosticCredential,
    );
    const report = parseDiagnosticReport(args.report);
    const payloadDigest = await diagnosticPayloadDigest(report);
    const existing = await ctx.db
      .query("diagnosticReports")
      .withIndex("by_reporterId_and_reportId", (q) =>
        q.eq("reporterId", reporter._id).eq("reportId", report.reportId),
      )
      .unique();
    if (existing) {
      if (existing.payloadDigest !== payloadDigest)
        throw new ConvexError("DIAGNOSTIC_REPORT_CONFLICT");
      return { outcome: "duplicate" as const, retryAfterMs: null };
    }
    const now = Date.now();
    assertDiagnosticReportAge(report, now);
    const limit = await rateLimiter.limit(ctx, "diagnosticSubmission", { key: reporter._id });
    if (!limit.ok) {
      return { outcome: "rate_limited" as const, retryAfterMs: Math.ceil(limit.retryAfter) };
    }
    await ctx.db.insert("diagnosticReports", {
      reporterId: reporter._id,
      tokenmaxxerId: reporter.tokenmaxxerId,
      deviceId: reporter.deviceId,
      generation: reporter.generation,
      reportId: report.reportId,
      payloadDigest,
      groupKey: await diagnosticGroupKey(report),
      provider: report.failure.provider,
      receivedAt: now,
      expiresAt: now + DIAGNOSTIC_SERVER_RETENTION_MS,
      report,
    });
    return { outcome: "accepted" as const, retryAfterMs: null };
  },
});

// These reads require deployment administrator access. They are not public client APIs.
export const forTouchGrassId = internalQuery({
  args: { touchGrassId: v.string(), paginationOpts: paginationOptsValidator },
  returns: paginationResultValidator(diagnosticStoredReportValidator),
  handler: async (ctx, args) => {
    const profile = await ctx.db
      .query("tokenmaxxers")
      .withIndex("by_public_id", (q) => q.eq("publicId", args.touchGrassId))
      .unique();
    if (!profile) return { page: [], isDone: true, continueCursor: "" };
    return ctx.db
      .query("diagnosticReports")
      .withIndex("by_tokenmaxxerId_and_receivedAt", (q) => q.eq("tokenmaxxerId", profile._id))
      .order("desc")
      .paginate(args.paginationOpts);
  },
});

export const forProfile = internalQuery({
  args: { tokenmaxxerId: v.id("tokenmaxxers"), paginationOpts: paginationOptsValidator },
  returns: paginationResultValidator(diagnosticStoredReportValidator),
  handler: (ctx, args) =>
    ctx.db
      .query("diagnosticReports")
      .withIndex("by_tokenmaxxerId_and_receivedAt", (q) =>
        q.eq("tokenmaxxerId", args.tokenmaxxerId),
      )
      .order("desc")
      .paginate(args.paginationOpts),
});

export const forDevice = internalQuery({
  args: { deviceId: v.id("devices"), paginationOpts: paginationOptsValidator },
  returns: paginationResultValidator(diagnosticStoredReportValidator),
  handler: (ctx, args) =>
    ctx.db
      .query("diagnosticReports")
      .withIndex("by_deviceId_and_receivedAt", (q) => q.eq("deviceId", args.deviceId))
      .order("desc")
      .paginate(args.paginationOpts),
});

export const forGroup = internalQuery({
  args: { groupKey: v.string(), paginationOpts: paginationOptsValidator },
  returns: paginationResultValidator(diagnosticStoredReportValidator),
  handler: (ctx, args) =>
    ctx.db
      .query("diagnosticReports")
      .withIndex("by_groupKey_and_receivedAt", (q) => q.eq("groupKey", args.groupKey))
      .order("desc")
      .paginate(args.paginationOpts),
});

export const forProvider = internalQuery({
  args: { provider: v.union(providerValidator, v.null()), paginationOpts: paginationOptsValidator },
  returns: paginationResultValidator(diagnosticStoredReportValidator),
  handler: (ctx, args) =>
    ctx.db
      .query("diagnosticReports")
      .withIndex("by_provider_and_receivedAt", (q) => q.eq("provider", args.provider))
      .order("desc")
      .paginate(args.paginationOpts),
});

export const revokeReporter = internalMutation({
  args: { reporterId: v.id("diagnosticReporters") },
  returns: v.null(),
  handler: async (ctx, args) => {
    const reporter = await ctx.db.get(args.reporterId);
    if (reporter && reporter.revokedAt === null)
      await ctx.db.patch(reporter._id, { revokedAt: Date.now() });
    return null;
  },
});

export const deleteExpired = internalMutation({
  args: {},
  returns: v.number(),
  handler: async (ctx): Promise<number> => {
    // 100 reports at 32 KiB is at most 3.2 MiB per transaction.
    const expired = await ctx.db
      .query("diagnosticReports")
      .withIndex("by_expiresAt", (q) => q.lte("expiresAt", Date.now()))
      .take(100);
    for (const report of expired) await ctx.db.delete(report._id);
    if (expired.length === 100)
      await ctx.scheduler.runAfter(0, internal.diagnostics.deleteExpired, {});
    return expired.length;
  },
});
