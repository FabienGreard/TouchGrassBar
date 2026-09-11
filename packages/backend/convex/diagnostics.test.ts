/// <reference types="vite/client" />

import betterAuthTest from "@convex-dev/better-auth/test";
import doomerboardIndexTest from "@convex-dev/aggregate/test";
import rateLimiterTest from "@convex-dev/rate-limiter/test";
import {
  DIAGNOSTIC_LOCAL_MAX_AGE_MS,
  DIAGNOSTIC_SERVER_RETENTION_MS,
  diagnosticReportSchema,
} from "@touchgrass/contracts";
import { convexTest } from "convex-test";
import { afterEach, beforeEach, expect, test, vi } from "vitest";

import fixtureText from "../../contracts/fixtures/diagnostic-reports-v1.json?raw";
import { api, internal } from "./_generated/api";
import { createAuthWithRequestIp } from "./auth";
import {
  diagnosticCredentialDigest,
  diagnosticPayloadDigest,
  parseDiagnosticReport,
} from "./model/diagnostics";
import { installationCredentialDigest } from "./model/profile";
import schema from "./schema";

const modules = import.meta.glob("./**/*.ts");
const NOW = Date.parse("2026-09-11T12:00:00.000Z");
const INSTALLATION_CREDENTIAL = "A".repeat(52);
const DIAGNOSTIC_CREDENTIAL = "a".repeat(64);
const fixtures = (JSON.parse(fixtureText) as unknown[]).map((value) =>
  diagnosticReportSchema.parse(value),
);

function testBackend() {
  const t = convexTest(schema, modules);
  betterAuthTest.register(t);
  doomerboardIndexTest.register(t, "doomerboard");
  rateLimiterTest.register(t);
  return t;
}

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(NOW);
  vi.stubEnv("BETTER_AUTH_SECRET", `${crypto.randomUUID()}${crypto.randomUUID()}`);
  vi.stubEnv("CONVEX_SITE_URL", "https://example.convex.site");
});
afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllEnvs();
});

async function seedReporter(t: ReturnType<typeof testBackend>) {
  return t.run(async (ctx) => {
    const tokenmaxxerId = await ctx.db.insert("tokenmaxxers", {
      activeAuthSessionId: "session",
      authSessionGeneration: 1,
      authSubject: "fixture",
      displayName: "Fixture",
      publicId: "TG-AAAAAA",
      createdAt: NOW,
    });
    const deviceId = await ctx.db.insert("devices", {
      tokenmaxxerId,
      generation: 1,
      createdAt: NOW,
      lastSeenAt: NOW,
      installationCredentialDigest: await installationCredentialDigest(INSTALLATION_CREDENTIAL),
    });
    await ctx.db.patch(tokenmaxxerId, { activeDeviceId: deviceId });
    const reporterId = await ctx.db.insert("diagnosticReporters", {
      tokenmaxxerId,
      deviceId,
      generation: 1,
      createdAt: NOW,
      revokedAt: null,
      credentialDigest: await diagnosticCredentialDigest(DIAGNOSTIC_CREDENTIAL),
    });
    return { tokenmaxxerId, deviceId, reporterId };
  });
}

async function authenticatedProfile(t: ReturnType<typeof testBackend>) {
  async function authRequest(path: string, init: RequestInit) {
    return t.action(async (ctx) => {
      const auth = createAuthWithRequestIp(ctx, async () => "203.0.113.24");
      const response = await auth.handler(new Request(`https://example.convex.site${path}`, init));
      expect(response.status).toBe(200);
      return (await response.json()) as Record<string, unknown>;
    });
  }
  const prepared = await authRequest("/api/auth/touchgrass/prepare", { method: "POST" });
  const touchGrassId = String(prepared.touchGrassId);
  const password = "R".repeat(48);
  const signup = await authRequest("/api/auth/sign-up/email", {
    method: "POST",
    headers: {
      "content-type": "application/json",
      "x-touchgrass-signup-proof": String(prepared.signupProof),
    },
    body: JSON.stringify({
      email: `${touchGrassId.toLowerCase()}@profile.touchgrass.invalid`,
      name: "Fixture",
      password,
      username: touchGrassId,
    }),
  });
  const userId = (signup.user as { id: string }).id;
  const session = await authRequest("/api/auth/sign-in/username", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ password, username: touchGrassId }),
  });
  const token = await authRequest("/api/auth/convex/token", {
    headers: { authorization: `Bearer ${String(session.token)}` },
  });
  const payload = String(token.token).split(".")[1]!;
  const normalized = payload.replaceAll("-", "+").replaceAll("_", "/");
  const claims = JSON.parse(atob(normalized.padEnd(Math.ceil(normalized.length / 4) * 4, "="))) as {
    sessionId: string;
  };
  const authenticated = t.withIdentity({
    subject: userId,
    tokenIdentifier: `touchgrass|${userId}`,
    sessionId: claims.sessionId,
  });
  await authenticated.mutation(api.tokenmaxxers.ensureProfile, {
    displayName: "Fixture",
    expectedTouchGrassId: touchGrassId,
    installationCredential: INSTALLATION_CREDENTIAL,
  });
  return authenticated;
}

test("registration needs current profile session, installation credential and generation; rotation keeps report identity", async () => {
  const t = testBackend();
  const args = {
    installationCredential: INSTALLATION_CREDENTIAL,
    activeMacGeneration: 1,
    diagnosticCredential: DIAGNOSTIC_CREDENTIAL,
  };
  await expect(t.mutation(api.diagnostics.register, args)).rejects.toThrow("authority-rejected");
  const authenticated = await authenticatedProfile(t);
  await expect(
    authenticated.mutation(api.diagnostics.register, { ...args, activeMacGeneration: 2 }),
  ).rejects.toThrow("authority-rejected");
  await expect(
    authenticated.mutation(api.diagnostics.register, {
      ...args,
      installationCredential: "B".repeat(52),
    }),
  ).rejects.toThrow("authority-rejected");
  const first = await authenticated.mutation(api.diagnostics.register, args);
  expect(await authenticated.mutation(api.diagnostics.register, args)).toEqual(first);
  const report = fixtures[0]!;
  await t.mutation(api.diagnostics.submit, {
    reporterId: first.reporterId,
    diagnosticCredential: DIAGNOSTIC_CREDENTIAL,
    report,
  });
  const rotated = "b".repeat(64);
  expect(
    await authenticated.mutation(api.diagnostics.register, {
      ...args,
      diagnosticCredential: rotated,
    }),
  ).toEqual(first);
  await expect(
    t.mutation(api.diagnostics.submit, {
      reporterId: first.reporterId,
      diagnosticCredential: DIAGNOSTIC_CREDENTIAL,
      report,
    }),
  ).rejects.toThrow("DIAGNOSTIC_AUTHORITY_REJECTED");
  expect(
    await t.mutation(api.diagnostics.submit, {
      reporterId: first.reporterId,
      diagnosticCredential: rotated,
      report,
    }),
  ).toEqual({ outcome: "duplicate", retryAfterMs: null });
  await t.mutation(internal.diagnostics.revokeReporter, { reporterId: first.reporterId });
  await expect(
    authenticated.mutation(api.diagnostics.register, { ...args, diagnosticCredential: rotated }),
  ).rejects.toThrow("DIAGNOSTIC_AUTHORITY_REJECTED");
});

test("anonymous narrow credential accepts all failure areas and assigns profile/device; reports do not update product health", async () => {
  const t = testBackend();
  const owner = await seedReporter(t);
  for (const report of fixtures) {
    expect(
      await t.mutation(api.diagnostics.submit, {
        reporterId: owner.reporterId,
        diagnosticCredential: DIAGNOSTIC_CREDENTIAL,
        report,
      }),
    ).toEqual({ outcome: "accepted", retryAfterMs: null });
  }
  const page = await t.query(internal.diagnostics.forProfile, {
    tokenmaxxerId: owner.tokenmaxxerId,
    paginationOpts: { numItems: 10, cursor: null },
  });
  expect(page.page).toHaveLength(fixtures.length);
  for (const row of page.page) {
    expect(row).toMatchObject({
      ...owner,
      generation: 1,
      receivedAt: NOW,
      expiresAt: NOW + DIAGNOSTIC_SERVER_RETENTION_MS,
    });
    expect(row.payloadDigest).toMatch(/^sha256:[0-9a-f]{64}$/);
  }
  await t.run(async (ctx) => {
    expect((await ctx.db.get(owner.tokenmaxxerId))?.lastSyncedAt).toBeUndefined();
    expect((await ctx.db.get(owner.deviceId))?.lastSeenAt).toBe(NOW);
    expect(
      await ctx.db
        .query("usageBuckets")
        .withIndex("by_device_id", (q) => q.eq("deviceId", owner.deviceId))
        .take(1),
    ).toEqual([]);
  });
  expect(
    (
      await t.query(internal.diagnostics.forProvider, {
        provider: "codex",
        paginationOpts: { numItems: 10, cursor: null },
      })
    ).page,
  ).toHaveLength(3);
  expect(
    (
      await t.query(internal.diagnostics.forTouchGrassId, {
        touchGrassId: "TG-AAAAAA",
        paginationOpts: { numItems: 10, cursor: null },
      })
    ).page,
  ).toHaveLength(fixtures.length);
  expect(
    (
      await t.query(internal.diagnostics.forTouchGrassId, {
        touchGrassId: "TG-BBBBBB",
        paginationOpts: { numItems: 10, cursor: null },
      })
    ).page,
  ).toEqual([]);
});

test.each([
  "secret",
  "reporter_revoked",
  "device_revoked",
  "generation",
  "active_device",
  "profile_generation",
])("submission rejects %s authority failure including retries", async (scenario) => {
  const t = testBackend();
  const owner = await seedReporter(t);
  const args = {
    reporterId: owner.reporterId,
    diagnosticCredential: DIAGNOSTIC_CREDENTIAL,
    report: fixtures[0]!,
  };
  await t.mutation(api.diagnostics.submit, args);
  await t.run(async (ctx) => {
    if (scenario === "reporter_revoked") await ctx.db.patch(owner.reporterId, { revokedAt: NOW });
    if (scenario === "device_revoked") await ctx.db.patch(owner.deviceId, { revokedAt: NOW });
    if (scenario === "generation") await ctx.db.patch(owner.deviceId, { generation: 2 });
    if (scenario === "active_device")
      await ctx.db.patch(owner.tokenmaxxerId, { activeDeviceId: undefined });
    if (scenario === "profile_generation")
      await ctx.db.patch(owner.tokenmaxxerId, { authSessionGeneration: 2 });
  });
  await expect(
    t.mutation(api.diagnostics.submit, {
      ...args,
      diagnosticCredential: scenario === "secret" ? "b".repeat(64) : args.diagnosticCredential,
    }),
  ).rejects.toThrow("DIAGNOSTIC_AUTHORITY_REJECTED");
});

test("frozen retry is idempotent before rate limit and age checks; changed payload is rejected", async () => {
  const t = testBackend();
  const { reporterId } = await seedReporter(t);
  const report = fixtures[0]!;
  const args = { reporterId, diagnosticCredential: DIAGNOSTIC_CREDENTIAL, report };
  for (let index = 0; index < 10; index++) {
    await t.mutation(api.diagnostics.submit, {
      ...args,
      report: { ...report, reportId: `00000000-0000-4000-9000-${String(index).padStart(12, "0")}` },
    });
  }
  const throttled = await t.mutation(api.diagnostics.submit, args);
  expect(throttled.outcome).toBe("rate_limited");
  expect(throttled.retryAfterMs).toBeGreaterThan(0);
  const accepted = {
    ...args,
    report: { ...report, reportId: "00000000-0000-4000-9000-000000000000" },
  };
  expect(await t.mutation(api.diagnostics.submit, accepted)).toEqual({
    outcome: "duplicate",
    retryAfterMs: null,
  });
  await expect(
    t.mutation(api.diagnostics.submit, {
      ...accepted,
      report: { ...accepted.report, occurrenceCount: 3 },
    }),
  ).rejects.toThrow("DIAGNOSTIC_REPORT_CONFLICT");
  vi.setSystemTime(NOW + DIAGNOSTIC_LOCAL_MAX_AGE_MS + 1);
  expect(await t.mutation(api.diagnostics.submit, accepted)).toEqual({
    outcome: "duplicate",
    retryAfterMs: null,
  });
  await expect(t.mutation(api.diagnostics.submit, args)).rejects.toThrow(
    "DIAGNOSTIC_REPORT_EXPIRED",
  );
});

test("strict report boundary rejects raw data, impossible values, success events, clock skew and oversized payloads", async () => {
  const t = testBackend();
  const { reporterId } = await seedReporter(t);
  const report = fixtures[0]!;
  const args = { reporterId, diagnosticCredential: DIAGNOSTIC_CREDENTIAL, report };
  await expect(
    t.mutation(api.diagnostics.submit, {
      ...args,
      report: { ...report, app: { ...report.app, build: "/Users/private/path" } },
    }),
  ).rejects.toThrow("DIAGNOSTIC_REPORT_INVALID");
  await expect(
    t.mutation(api.diagnostics.submit, { ...args, report: { ...report, occurrenceCount: NaN } }),
  ).rejects.toThrow("DIAGNOSTIC_REPORT_INVALID");
  await expect(
    t.mutation(api.diagnostics.submit, {
      ...args,
      report: { ...report, lastOccurredAt: NOW + 300_001 },
    }),
  ).rejects.toThrow("DIAGNOSTIC_REPORT_EXPIRED");
  expect(() =>
    parseDiagnosticReport({
      ...report,
      failure: { area: "healthy", provider: null, code: "ok", context: {} },
    }),
  ).toThrow("DIAGNOSTIC_REPORT_INVALID");
  expect(() => parseDiagnosticReport({ ...report, rawLogs: "x".repeat(32 * 1024) })).toThrow(
    "DIAGNOSTIC_REPORT_TOO_LARGE",
  );
  expect(() => parseDiagnosticReport({ ...report, rawLogs: "private text" })).toThrow(
    "DIAGNOSTIC_REPORT_INVALID",
  );
  expect(() =>
    parseDiagnosticReport({ ...report, app: { ...report.app, version: "1".repeat(33) } }),
  ).toThrow("DIAGNOSTIC_REPORT_INVALID");
  expect(() =>
    parseDiagnosticReport({ ...report, firstOccurredAt: NOW, lastOccurredAt: NOW - 1 }),
  ).toThrow("DIAGNOSTIC_REPORT_INVALID");
});

test("canonical digest ignores object key order and grouping preserves actual failure provenance", async () => {
  const report = fixtures[0]!;
  const entries = Object.entries(report);
  const reordered = Object.fromEntries(entries.slice(1).concat(entries.slice(0, 1)));
  expect(await diagnosticPayloadDigest(parseDiagnosticReport(reordered))).toBe(
    await diagnosticPayloadDigest(report),
  );
  const t = testBackend();
  const owner = await seedReporter(t);
  const args = { reporterId: owner.reporterId, diagnosticCredential: DIAGNOSTIC_CREDENTIAL };
  await t.mutation(api.diagnostics.submit, { ...args, report });
  await t.mutation(api.diagnostics.submit, {
    ...args,
    report: { ...report, reportId: crypto.randomUUID(), occurrenceCount: 4 },
  });
  const page = await t.query(internal.diagnostics.forDevice, {
    deviceId: owner.deviceId,
    paginationOpts: { numItems: 10, cursor: null },
  });
  expect(page.page[0]!.groupKey).toBe(page.page[1]!.groupKey);
  expect(
    (
      await t.query(internal.diagnostics.forGroup, {
        groupKey: page.page[0]!.groupKey,
        paginationOpts: { numItems: 10, cursor: null },
      })
    ).page,
  ).toHaveLength(2);
});

test("an empty backfill failure has no provider and cannot claim a provider revision", async () => {
  const t = testBackend();
  const { reporterId } = await seedReporter(t);
  const report = fixtures[7]!;
  if (report.failure.area !== "sync") throw new Error("Expected empty backfill fixture");
  const args = { reporterId, diagnosticCredential: DIAGNOSTIC_CREDENTIAL, report };
  expect(await t.mutation(api.diagnostics.submit, args)).toEqual({
    outcome: "accepted",
    retryAfterMs: null,
  });
  for (const override of [
    { rankingDay: "2026-09-11" },
    { attemptedRevision: 1 },
    { lastAcknowledgedRevision: 1 },
    { pendingCount: 1 },
  ]) {
    await expect(
      t.mutation(api.diagnostics.submit, {
        ...args,
        report: {
          ...report,
          reportId: crypto.randomUUID(),
          failure: { ...report.failure, context: { ...report.failure.context, ...override } },
        },
      }),
    ).rejects.toThrow("DIAGNOSTIC_REPORT_INVALID");
  }
});

test("retention deletes expired reports in bounded batches and keeps fresh reports", async () => {
  const t = testBackend();
  const owner = await seedReporter(t);
  await t.mutation(api.diagnostics.submit, {
    reporterId: owner.reporterId,
    diagnosticCredential: DIAGNOSTIC_CREDENTIAL,
    report: fixtures[0]!,
  });
  await t.run(async (ctx) => {
    const row = (
      await ctx.db
        .query("diagnosticReports")
        .withIndex("by_reporterId_and_reportId", (q) => q.eq("reporterId", owner.reporterId))
        .take(1)
    )[0]!;
    const { _id: ignoredId, _creationTime: ignoredCreationTime, ...data } = row;
    void ignoredId;
    void ignoredCreationTime;
    for (let index = 0; index < 101; index++) {
      await ctx.db.insert("diagnosticReports", {
        ...data,
        reportId: crypto.randomUUID(),
        expiresAt: NOW - 1,
      });
    }
  });
  expect(await t.mutation(internal.diagnostics.deleteExpired, {})).toBe(100);
  await t.finishAllScheduledFunctions(() => vi.runAllTimers());
  const page = await t.query(internal.diagnostics.forProfile, {
    tokenmaxxerId: owner.tokenmaxxerId,
    paginationOpts: { numItems: 10, cursor: null },
  });
  expect(page.page).toHaveLength(1);
  expect(page.page[0]!.expiresAt).toBe(NOW + DIAGNOSTIC_SERVER_RETENTION_MS);
});
