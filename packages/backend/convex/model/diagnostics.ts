import {
  DIAGNOSTIC_LOCAL_MAX_AGE_MS,
  DIAGNOSTIC_MAX_BYTES,
  DIAGNOSTIC_MAX_FUTURE_SKEW_MS,
  diagnosticReportSchema,
  type DiagnosticReport,
} from "@touchgrass/contracts";
import { ConvexError } from "convex/values";

import type { Id } from "../_generated/dataModel";
import type { MutationCtx } from "../_generated/server";

export async function diagnosticDigest(value: string): Promise<string> {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(value));
  return `sha256:${Array.from(new Uint8Array(digest), (byte) => byte.toString(16).padStart(2, "0")).join("")}`;
}

export function assertDiagnosticCredential(credential: string) {
  if (!/^[a-f0-9]{64}$/.test(credential)) {
    throw new ConvexError("DIAGNOSTIC_AUTHORITY_REJECTED");
  }
}

export function diagnosticCredentialDigest(credential: string) {
  assertDiagnosticCredential(credential);
  return diagnosticDigest(`touchgrass-diagnostic-credential-v1:${credential}`);
}

function canonical(value: unknown): string {
  if (value === null || typeof value !== "object") return JSON.stringify(value);
  if (Array.isArray(value)) return `[${value.map(canonical).join(",")}]`;
  const object = value as Record<string, unknown>;
  return `{${Object.keys(object)
    .sort()
    .map((key) => `${JSON.stringify(key)}:${canonical(object[key])}`)
    .join(",")}}`;
}

export function parseDiagnosticReport(value: unknown): DiagnosticReport {
  const encoded = JSON.stringify(value);
  if (
    encoded === undefined ||
    new TextEncoder().encode(encoded).byteLength > DIAGNOSTIC_MAX_BYTES
  ) {
    throw new ConvexError("DIAGNOSTIC_REPORT_TOO_LARGE");
  }
  const parsed = diagnosticReportSchema.safeParse(value);
  if (!parsed.success) throw new ConvexError("DIAGNOSTIC_REPORT_INVALID");
  return parsed.data;
}

export function assertDiagnosticReportAge(report: DiagnosticReport, now: number) {
  if (
    report.firstOccurredAt < now - DIAGNOSTIC_LOCAL_MAX_AGE_MS ||
    report.lastOccurredAt > now + DIAGNOSTIC_MAX_FUTURE_SKEW_MS ||
    report.contextCapturedAt > now + DIAGNOSTIC_MAX_FUTURE_SKEW_MS
  ) {
    throw new ConvexError("DIAGNOSTIC_REPORT_EXPIRED");
  }
}

export function diagnosticPayloadDigest(report: DiagnosticReport) {
  return diagnosticDigest(canonical(report));
}

export function diagnosticGroupKey(report: DiagnosticReport) {
  const failure = report.failure;
  const detail = failure.area === "database" ? failure.context.stage : failure.context.reason;
  // Exclude profile, timestamps, counts and day so support can group the same failure.
  return diagnosticDigest(
    canonical({
      schemaVersion: report.schemaVersion,
      area: failure.area,
      code: failure.code,
      provider: failure.provider,
      detail,
      appVersion: report.app.version,
      parserVersion:
        failure.area === "parser" || failure.area === "pricing"
          ? failure.context.parserVersion
          : null,
      catalogVersion: failure.area === "pricing" ? failure.context.catalogVersion : null,
    }),
  );
}

export async function requireDiagnosticReporter(
  ctx: MutationCtx,
  reporterId: Id<"diagnosticReporters">,
  credential: string,
) {
  const digest = await diagnosticCredentialDigest(credential);
  const reporter = await ctx.db.get(reporterId);
  if (!reporter || reporter.revokedAt !== null || reporter.credentialDigest !== digest) {
    throw new ConvexError("DIAGNOSTIC_AUTHORITY_REJECTED");
  }
  const [device, profile] = await Promise.all([
    ctx.db.get(reporter.deviceId),
    ctx.db.get(reporter.tokenmaxxerId),
  ]);
  if (
    !device ||
    !profile ||
    device.revokedAt !== undefined ||
    device.tokenmaxxerId !== reporter.tokenmaxxerId ||
    device.generation !== reporter.generation ||
    profile.activeDeviceId !== device._id ||
    profile.authSessionGeneration !== reporter.generation
  ) {
    throw new ConvexError("DIAGNOSTIC_AUTHORITY_REJECTED");
  }
  if (profile.recoveryAttemptId) {
    const recovery = await ctx.db.get(profile.recoveryAttemptId);
    if (recovery?.status === "committed" && recovery.authFinalizedAt === undefined) {
      throw new ConvexError("DIAGNOSTIC_AUTHORITY_REJECTED");
    }
  }
  return reporter;
}
