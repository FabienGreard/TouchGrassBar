/// <reference types="vite/client" />
import betterAuthTest from "@convex-dev/better-auth/test";
import doomerboardIndexTest from "@convex-dev/aggregate/test";
import rateLimiterTest from "@convex-dev/rate-limiter/test";
import { convexTest } from "convex-test";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { supportReportSchema } from "@touchgrass/contracts";
import fixtureText from "../../contracts/fixtures/diagnostic-reports-v1.json?raw";
import { api, internal } from "./_generated/api";
import { createAuthWithRequestIp } from "./auth";
import schema from "./schema";
const modules = import.meta.glob("./**/*.ts");
const NOW = Date.parse("2026-09-24T12:00:00.000Z");
const INSTALLATION_CREDENTIAL = "A".repeat(52);
const authArgs = { installationCredential: INSTALLATION_CREDENTIAL, activeMacGeneration: 1 };
const scan = JSON.parse(fixtureText).find(
  (item: { failure: { context: { reason?: string } } }) =>
    item.failure.context.reason === "scan_incomplete",
).failure.context.scan;
function report() {
  return supportReportSchema.parse({
    schemaVersion: 1,
    appVersion: "0.0.53",
    capturedAt: Date.now(),
    providers: [
      { provider: "codex", scan },
      { provider: "claude", scan },
    ],
  });
}
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
  return { authenticated, touchGrassId };
}

test("remote report requires consent; the current Mac returns an immutable bounded result", async () => {
  const t = testBackend();
  await expect(t.mutation(api.support.start, authArgs)).rejects.toThrow("authority-rejected");
  const { authenticated, touchGrassId } = await authenticatedProfile(t);
  await expect(
    t.mutation(internal.support.requestReport, { touchGrassId, operator: "test" }),
  ).rejects.toThrow("SUPPORT_SESSION_UNAVAILABLE");
  await expect(
    authenticated.mutation(api.support.start, { ...authArgs, activeMacGeneration: 2 }),
  ).rejects.toThrow("authority-rejected");
  const session = await authenticated.mutation(api.support.start, authArgs);
  expect(session.expiresAt).toBe(NOW + 30 * 60_000);
  expect(await authenticated.mutation(api.support.start, authArgs)).toEqual(session);
  const sessionArgs = { ...authArgs, sessionId: session.sessionId };
  const requestId = await t.mutation(internal.support.requestReport, {
    touchGrassId,
    operator: "test",
  });
  expect(await t.mutation(internal.support.requestReport, { touchGrassId, operator: "test" })).toBe(
    requestId,
  );
  expect(await authenticated.mutation(api.support.poll, sessionArgs)).toEqual({
    active: true,
    requestId,
  });
  await authenticated.mutation(api.support.complete, {
    ...sessionArgs,
    requestId,
    report: report(),
  });
  await authenticated.mutation(api.support.complete, { ...sessionArgs, requestId, report: null });
  const result = await t.query(internal.support.forTouchGrassId, { touchGrassId });
  expect(result.requests[0]?.report).toEqual(report());
  expect(result.requests[0]?.operator).toBe("test");
  expect(await authenticated.mutation(api.support.poll, sessionArgs)).toEqual({
    active: true,
    requestId: null,
  });
  await expect(
    t.mutation(internal.support.requestReport, { touchGrassId, operator: "test" }),
  ).rejects.toThrow("SUPPORT_RATE_LIMITED");
});

test("another profile or installation cannot poll, cancel, or complete a session", async () => {
  const t = testBackend();
  const first = await authenticatedProfile(t);
  const second = await authenticatedProfile(t);
  const session = await first.authenticated.mutation(api.support.start, authArgs);
  const args = { ...authArgs, sessionId: session.sessionId };
  for (const fn of [api.support.poll, api.support.cancel]) {
    await expect(second.authenticated.mutation(fn, args)).rejects.toThrow(
      "SUPPORT_AUTHORITY_REJECTED",
    );
    await expect(
      first.authenticated.mutation(fn, { ...args, installationCredential: "B".repeat(52) }),
    ).rejects.toThrow("authority-rejected");
  }
  const requestId = await t.mutation(internal.support.requestReport, {
    touchGrassId: first.touchGrassId,
    operator: "test",
  });
  await expect(
    second.authenticated.mutation(api.support.complete, { ...args, requestId, report: report() }),
  ).rejects.toThrow("SUPPORT_AUTHORITY_REJECTED");
});

test("request deadline, cancellation and session expiry reject late reports", async () => {
  const t = testBackend();
  const { authenticated, touchGrassId } = await authenticatedProfile(t);
  const session = await authenticated.mutation(api.support.start, authArgs);
  const args = { ...authArgs, sessionId: session.sessionId };
  const requestId = await t.mutation(internal.support.requestReport, {
    touchGrassId,
    operator: "test",
  });
  vi.setSystemTime(NOW + 2 * 60_000);
  expect((await authenticated.mutation(api.support.poll, args)).requestId).toBeNull();
  await expect(
    authenticated.mutation(api.support.complete, { ...args, requestId, report: report() }),
  ).rejects.toThrow("SUPPORT_REQUEST_CLOSED");
  const fresh = await t.mutation(internal.support.requestReport, {
    touchGrassId,
    operator: "test",
  });
  await authenticated.mutation(api.support.cancel, args);
  expect((await authenticated.mutation(api.support.poll, args)).active).toBe(false);
  await expect(
    authenticated.mutation(api.support.complete, { ...args, requestId: fresh, report: report() }),
  ).rejects.toThrow("SUPPORT_REQUEST_CLOSED");
  await expect(
    t.mutation(internal.support.requestReport, { touchGrassId, operator: "test" }),
  ).rejects.toThrow("SUPPORT_SESSION_UNAVAILABLE");
  const next = await authenticated.mutation(api.support.start, authArgs);
  vi.setSystemTime(next.expiresAt);
  expect(
    (await authenticated.mutation(api.support.poll, { ...authArgs, sessionId: next.sessionId }))
      .active,
  ).toBe(false);
});

test("recovery or device revocation invalidates remote support authority", async () => {
  const t = testBackend();
  const { authenticated, touchGrassId } = await authenticatedProfile(t);
  const session = await authenticated.mutation(api.support.start, authArgs);
  await t.run(async (ctx) => {
    const stored = await ctx.db.get(session.sessionId);
    await ctx.db.patch(stored!.deviceId, { revokedAt: NOW });
  });
  await expect(
    authenticated.mutation(api.support.poll, { ...authArgs, sessionId: session.sessionId }),
  ).rejects.toThrow("authority-rejected");
  await expect(
    t.mutation(internal.support.requestReport, { touchGrassId, operator: "test" }),
  ).rejects.toThrow("SUPPORT_SESSION_UNAVAILABLE");
});

test("reject private and stale reports; record unavailable reads and remove expired records", async () => {
  const t = testBackend();
  const { authenticated, touchGrassId } = await authenticatedProfile(t);
  const session = await authenticated.mutation(api.support.start, authArgs);
  const requestId = await t.mutation(internal.support.requestReport, {
    touchGrassId,
    operator: "test",
  });
  const args = { ...authArgs, sessionId: session.sessionId, requestId };
  await expect(
    authenticated.mutation(api.support.complete, {
      ...args,
      report: { ...report(), capturedAt: NOW - 60_001 },
    }),
  ).rejects.toThrow("SUPPORT_REPORT_INVALID");
  await expect(
    authenticated.mutation(api.support.complete, {
      ...args,
      report: {
        ...report(),
        providers: [
          { provider: "codex", scan: { ...scan, privatePath: "/private/session" } },
          { provider: "claude", scan },
        ],
      },
    }),
  ).rejects.toThrow();
  await authenticated.mutation(api.support.complete, { ...args, report: null });
  expect(
    (await t.query(internal.support.forTouchGrassId, { touchGrassId })).requests[0]?.failure,
  ).toBe("report_unavailable");
  vi.setSystemTime(NOW + 8 * 24 * 60 * 60_000);
  await t.mutation(internal.support.deleteExpired, {});
  expect(await t.query(internal.support.forTouchGrassId, { touchGrassId })).toEqual({
    session: null,
    requests: [],
  });
});

test("a recovered generation cannot reuse the previous Mac's session", async () => {
  const t = testBackend();
  const { authenticated, touchGrassId } = await authenticatedProfile(t);
  const session = await authenticated.mutation(api.support.start, authArgs);
  await t.run(async (ctx) => {
    const stored = await ctx.db.get(session.sessionId);
    const previous = await ctx.db.get(stored!.deviceId);
    const deviceId = await ctx.db.insert("devices", {
      tokenmaxxerId: stored!.tokenmaxxerId,
      generation: 2,
      createdAt: NOW,
      lastSeenAt: NOW,
      installationCredentialDigest: previous!.installationCredentialDigest,
    });
    await ctx.db.patch(previous!._id, { revokedAt: NOW });
    await ctx.db.patch(stored!.tokenmaxxerId, {
      activeDeviceId: deviceId,
      authSessionGeneration: 2,
    });
  });
  await expect(
    authenticated.mutation(api.support.poll, {
      ...authArgs,
      activeMacGeneration: 2,
      sessionId: session.sessionId,
    }),
  ).rejects.toThrow("SUPPORT_AUTHORITY_REJECTED");
  await expect(
    t.mutation(internal.support.requestReport, { touchGrassId, operator: "test" }),
  ).rejects.toThrow("SUPPORT_SESSION_UNAVAILABLE");
  const next = await authenticated.mutation(api.support.start, {
    ...authArgs,
    activeMacGeneration: 2,
  });
  expect(next.sessionId).not.toBe(session.sessionId);
  expect(
    await t.mutation(internal.support.requestReport, { touchGrassId, operator: "test" }),
  ).toBeTruthy();
});
