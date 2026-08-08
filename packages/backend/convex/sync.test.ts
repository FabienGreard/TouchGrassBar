/// <reference types="vite/client" />

import aggregateTest from "@convex-dev/aggregate/test";
import betterAuthTest from "@convex-dev/better-auth/test";
import {
  type MigrationFunctionReference,
  runToCompletion,
} from "@convex-dev/migrations";
import migrationsTest from "@convex-dev/migrations/test";
import rateLimiterTest from "@convex-dev/rate-limiter/test";
import { convexTest } from "convex-test";
import { afterEach, beforeEach, expect, test, vi } from "vitest";

import { api, components, internal } from "./_generated/api";
import { createAuthWithRequestIp } from "./auth";
import type { UsageSnapshot } from "./model/values";
import schema from "./schema";

const modules = import.meta.glob("./**/*.ts");
const TODAY = "2026-08-08";
const NOW = new Date(`${TODAY}T12:00:00.000Z`);

function testBackend() {
  const t = convexTest(schema, modules);
  aggregateTest.register(t, "doomerboard");
  betterAuthTest.register(t);
  migrationsTest.register(t);
  rateLimiterTest.register(t);
  return t;
}

async function authFetch(
  t: ReturnType<typeof testBackend>,
  path: string,
  init: RequestInit,
) {
  const result = await t.action(async (ctx) => {
    const auth = createAuthWithRequestIp(ctx, async () => "203.0.113.10");
    const response = await auth.handler(
      new Request(`https://example.convex.site${path}`, init),
    );
    return {
      body: await response.text(),
      headers: Array.from(response.headers.entries()),
      status: response.status,
    };
  });
  return new Response(result.body, {
    headers: result.headers,
    status: result.status,
  });
}

function bearer(token: string) {
  return { authorization: `Bearer ${token}` };
}

async function json(response: Response) {
  return (await response.json()) as Record<string, unknown>;
}

function jwtPayload(token: string) {
  const encoded = token.split(".")[1];
  if (!encoded) throw new Error("JWT payload is missing");
  const normalized = encoded.replaceAll("-", "+").replaceAll("_", "/");
  const padded = normalized.padEnd(Math.ceil(normalized.length / 4) * 4, "=");
  return JSON.parse(atob(padded)) as { sessionId?: unknown; sub?: unknown };
}

function installationCredential(character: string) {
  return character.repeat(52);
}

async function authenticateProfile(
  t: ReturnType<typeof testBackend>,
  displayName: string,
) {
  const preparation = await authFetch(t, "/api/auth/touchgrass/prepare", {
    method: "POST",
  });
  expect(preparation.status).toBe(200);
  const prepared = await json(preparation);
  const recoveryKey = `${crypto.randomUUID()}${crypto.randomUUID()}`;
  const signup = await authFetch(t, "/api/auth/sign-up/email", {
    body: JSON.stringify({
      email: `${String(prepared.touchGrassId).toLowerCase()}@profile.touchgrass.invalid`,
      name: displayName,
      password: recoveryKey,
      username: prepared.touchGrassId,
    }),
    headers: {
      "content-type": "application/json",
      "x-touchgrass-signup-proof": String(prepared.signupProof),
    },
    method: "POST",
  });
  expect(signup.status).toBe(200);
  const user = (await json(signup)).user as { id: string };
  const signIn = await authFetch(t, "/api/auth/sign-in/username", {
    body: JSON.stringify({
      password: recoveryKey,
      username: prepared.touchGrassId,
    }),
    headers: { "content-type": "application/json" },
    method: "POST",
  });
  expect(signIn.status).toBe(200);
  const { token: sessionToken } = (await json(signIn)) as { token: string };
  const tokenResponse = await authFetch(t, "/api/auth/convex/token", {
    headers: bearer(sessionToken),
  });
  expect(tokenResponse.status).toBe(200);
  const { token: convexJwt } = (await json(tokenResponse)) as { token: string };
  const payload = jwtPayload(convexJwt);
  const authenticated = t.withIdentity({
    sessionId: payload.sessionId as string,
    subject: user.id,
    tokenIdentifier: `touchgrass|${user.id}`,
  });
  return {
    authenticated,
    sessionToken,
    touchGrassId: String(prepared.touchGrassId),
  };
}

async function createProfile(
  t: ReturnType<typeof testBackend>,
  credential: string,
  displayName: string,
) {
  const profile = await authenticateProfile(t, displayName);
  await expect(
    profile.authenticated.mutation(api.tokenmaxxers.ensureProfile, {
      displayName,
      expectedTouchGrassId: profile.touchGrassId,
      installationCredential: credential,
    }),
  ).resolves.toEqual({
    activeMacGeneration: 1,
    displayName,
    touchGrassId: profile.touchGrassId,
  });
  return profile;
}

function usageSnapshot(
  overrides: Partial<UsageSnapshot> = {},
): UsageSnapshot {
  const evidenceBasis = overrides.evidenceBasis ?? "locally-derived";
  const provider = overrides.provider ?? "codex";
  return {
    apiEquivalentCost: {
      coveragePercent: null,
      micros: 1_000,
      pricingBasis:
        provider === "codex"
          ? "openai-standard-2026-08-06-v1"
          : "anthropic-standard-2026-08-07-v1",
      quality: evidenceBasis === "provider-reported" ? "reconciled" : "local-only",
    },
    correctionReason: null,
    correctionRevision: null,
    coverage: "complete",
    evidenceBasis,
    observedAt: NOW.getTime(),
    observedTokens: 100,
    provider,
    rankingDay: TODAY,
    revision: 1,
    ...overrides,
  };
}

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(NOW);
  vi.stubEnv(
    "BETTER_AUTH_SECRET",
    `${crypto.randomUUID()}${crypto.randomUUID()}`,
  );
  vi.stubEnv("CONVEX_SITE_URL", "https://example.convex.site");
});

afterEach(() => {
  vi.unstubAllEnvs();
  vi.useRealTimers();
});

test("both providers commit atomically and retries report exact revision outcomes", async () => {
  const t = testBackend();
  const credential = installationCredential("A");
  const { authenticated } = await createProfile(t, credential, "Fabien");
  const snapshots = [
    usageSnapshot(),
    usageSnapshot({
      apiEquivalentCost: null,
      evidenceBasis: "provider-reported",
      observedAt: NOW.getTime() + 5 * 60 * 1_000,
      observedTokens: 200,
      provider: "claude",
    }),
  ];

  await expect(
    authenticated.mutation(api.sync.dailyUsage, {
      activeMacGeneration: 1,
      installationCredential: credential,
      snapshots,
    }),
  ).resolves.toEqual([
    {
      outcome: "committed",
      provider: "codex",
      rankingDay: TODAY,
      revision: 1,
    },
    {
      outcome: "committed",
      provider: "claude",
      rankingDay: TODAY,
      revision: 1,
    },
  ]);
  const beforeRetry = await t.run(async (ctx) =>
    ctx.db.query("usageBuckets").collect(),
  );
  expect(beforeRetry).toHaveLength(2);
  expect(await t.run(async (ctx) => ctx.db.query("publicScores").collect())).toHaveLength(9);

  await expect(
    authenticated.mutation(api.sync.dailyUsage, {
      activeMacGeneration: 1,
      installationCredential: credential,
      snapshots,
    }),
  ).resolves.toEqual([
    {
      outcome: "idempotent",
      provider: "codex",
      rankingDay: TODAY,
      revision: 1,
    },
    {
      outcome: "idempotent",
      provider: "claude",
      rankingDay: TODAY,
      revision: 1,
    },
  ]);
  expect(await t.run(async (ctx) => ctx.db.query("usageBuckets").collect())).toEqual(
    beforeRetry,
  );

  await authenticated.mutation(api.sync.dailyUsage, {
    activeMacGeneration: 1,
    installationCredential: credential,
    snapshots: [usageSnapshot({ observedTokens: 110, revision: 3 })],
  });
  await expect(
    authenticated.mutation(api.sync.dailyUsage, {
      activeMacGeneration: 1,
      installationCredential: credential,
      snapshots: [usageSnapshot({ revision: 2 })],
    }),
  ).resolves.toEqual([
    {
      outcome: "stale",
      provider: "codex",
      rankingDay: TODAY,
      revision: 3,
    },
  ]);
});

test("Active Mac authority is isolated by Profile, credential, generation, and revocation", async () => {
  const t = testBackend();
  const aliceCredential = installationCredential("A");
  const bobCredential = installationCredential("B");
  const alice = await createProfile(t, aliceCredential, "Alice");
  const bob = await createProfile(t, bobCredential, "Bob");
  const snapshot = usageSnapshot();

  for (const attempt of [
    alice.authenticated.mutation(api.sync.dailyUsage, {
      activeMacGeneration: 1,
      installationCredential: bobCredential,
      snapshots: [snapshot],
    }),
    alice.authenticated.mutation(api.sync.dailyUsage, {
      activeMacGeneration: 2,
      installationCredential: aliceCredential,
      snapshots: [snapshot],
    }),
    bob.authenticated.mutation(api.sync.dailyUsage, {
      activeMacGeneration: 1,
      installationCredential: aliceCredential,
      snapshots: [snapshot],
    }),
  ]) {
    await expect(attempt).rejects.toThrow("authority-rejected");
  }

  await t.run(async (ctx) => {
    const tokenmaxxer = await ctx.db
      .query("tokenmaxxers")
      .withIndex("by_public_id", (q) => q.eq("publicId", alice.touchGrassId))
      .unique();
    if (!tokenmaxxer?.activeDeviceId) throw new Error("Active Mac missing");
    await ctx.db.patch(tokenmaxxer.activeDeviceId, { revokedAt: Date.now() });
  });
  await expect(
    alice.authenticated.mutation(api.sync.dailyUsage, {
      activeMacGeneration: 1,
      installationCredential: aliceCredential,
      snapshots: [snapshot],
    }),
  ).rejects.toThrow("authority-rejected");
  expect(await t.run(async (ctx) => ctx.db.query("usageBuckets").collect())).toEqual([]);
});

test("a mismatched live session cannot create or change Active Mac authority", async () => {
  const t = testBackend();
  const aliceCredential = installationCredential("A");
  const alice = await createProfile(t, aliceCredential, "Alice");
  const bob = await authenticateProfile(t, "Bob");
  expect(bob.touchGrassId).not.toBe(alice.touchGrassId);
  const before = await t.run(async (ctx) => ({
    devices: await ctx.db.query("devices").collect(),
    tokenmaxxers: await ctx.db.query("tokenmaxxers").collect(),
  }));

  await expect(
    bob.authenticated.mutation(api.tokenmaxxers.ensureProfile, {
      displayName: "Hostile change",
      expectedTouchGrassId: alice.touchGrassId,
      installationCredential: aliceCredential,
    }),
  ).rejects.toMatchObject({
    data: { code: "authority-rejected" },
    name: "ConvexError",
  });

  const after = await t.run(async (ctx) => ({
    devices: await ctx.db.query("devices").collect(),
    tokenmaxxers: await ctx.db.query("tokenmaxxers").collect(),
  }));
  expect(after).toEqual(before);
  expect(after.tokenmaxxers).toHaveLength(1);
  expect(after.tokenmaxxers[0]).toMatchObject({
    displayName: "Alice",
    publicId: alice.touchGrassId,
  });
  expect(after.devices).toHaveLength(1);
});

test("two Profiles commit both providers without crossing usage or score ownership", async () => {
  const t = testBackend();
  const aliceCredential = installationCredential("A");
  const bobCredential = installationCredential("B");
  const alice = await createProfile(t, aliceCredential, "Alice");
  const bob = await createProfile(t, bobCredential, "Bob");

  await alice.authenticated.mutation(api.sync.dailyUsage, {
    activeMacGeneration: 1,
    installationCredential: aliceCredential,
    snapshots: [
      usageSnapshot({ observedTokens: 101 }),
      usageSnapshot({ observedTokens: 102, provider: "claude" }),
    ],
  });
  await bob.authenticated.mutation(api.sync.dailyUsage, {
    activeMacGeneration: 1,
    installationCredential: bobCredential,
    snapshots: [
      usageSnapshot({ observedTokens: 201 }),
      usageSnapshot({ observedTokens: 202, provider: "claude" }),
    ],
  });

  const stored = await t.run(async (ctx) => ({
    publicScores: await ctx.db.query("publicScores").collect(),
    tokenmaxxers: await ctx.db.query("tokenmaxxers").collect(),
    usageBuckets: await ctx.db.query("usageBuckets").collect(),
    userDailyUsage: await ctx.db.query("userDailyUsage").collect(),
    userScores: await ctx.db.query("userScores").collect(),
  }));
  const owner = (touchGrassId: string) => {
    const tokenmaxxer = stored.tokenmaxxers.find(
      (candidate) => candidate.publicId === touchGrassId,
    );
    if (!tokenmaxxer) throw new Error("Profile missing");
    return tokenmaxxer._id;
  };
  const aliceId = owner(alice.touchGrassId);
  const bobId = owner(bob.touchGrassId);
  expect(
    stored.usageBuckets
      .filter((row) => row.tokenmaxxerId === aliceId)
      .map((row) => row.observedTokens)
      .sort((left, right) => left - right),
  ).toEqual([101, 102]);
  expect(
    stored.usageBuckets
      .filter((row) => row.tokenmaxxerId === bobId)
      .map((row) => row.observedTokens)
      .sort((left, right) => left - right),
  ).toEqual([201, 202]);
  expect(stored.userDailyUsage).toHaveLength(4);
  expect(stored.userScores).toHaveLength(18);
  expect(stored.publicScores).toHaveLength(18);
  const serialized = JSON.stringify(stored);
  expect(serialized).not.toContain(aliceCredential);
  expect(serialized).not.toContain(bobCredential);
});

test("a revoked Better Auth session cannot synchronize", async () => {
  const t = testBackend();
  const credential = installationCredential("A");
  const profile = await createProfile(t, credential, "Fabien");
  const signOut = await authFetch(t, "/api/auth/sign-out", {
    headers: bearer(profile.sessionToken),
    method: "POST",
  });
  expect(signOut.status).toBe(200);

  await expect(
    profile.authenticated.mutation(api.sync.dailyUsage, {
      activeMacGeneration: 1,
      installationCredential: credential,
      snapshots: [usageSnapshot()],
    }),
  ).rejects.toThrow("authority-rejected");
});

test("a legacy Active Mac cannot bind an unproved replacement credential", async () => {
  const t = testBackend();
  const credential = installationCredential("A");
  const profile = await createProfile(t, credential, "Fabien");
  await t.run(async (ctx) => {
    const tokenmaxxer = await ctx.db
      .query("tokenmaxxers")
      .withIndex("by_public_id", (q) => q.eq("publicId", profile.touchGrassId))
      .unique();
    if (!tokenmaxxer?.activeDeviceId) throw new Error("Active Mac missing");
    await ctx.db.patch(tokenmaxxer.activeDeviceId, {
      generation: undefined,
      installationCredentialDigest: undefined,
      installationId: "legacy-unproved-installation",
    });
  });

  await expect(
    profile.authenticated.mutation(api.tokenmaxxers.ensureProfile, {
      displayName: "Fabien",
      expectedTouchGrassId: profile.touchGrassId,
      installationCredential: credential,
    }),
  ).rejects.toThrow("authority-rejected");
  await expect(
    profile.authenticated.mutation(api.sync.dailyUsage, {
      activeMacGeneration: 1,
      installationCredential: credential,
      snapshots: [usageSnapshot()],
    }),
  ).rejects.toThrow("authority-rejected");
});

test("the governed legacy-device migration enables a fresh proved authority claim", async () => {
  const t = testBackend();
  const credential = installationCredential("A");
  const profile = await createProfile(t, credential, "Fabien");
  const legacyDeviceId = await t.run(async (ctx) => {
    const tokenmaxxer = await ctx.db
      .query("tokenmaxxers")
      .withIndex("by_public_id", (q) => q.eq("publicId", profile.touchGrassId))
      .unique();
    if (!tokenmaxxer?.activeDeviceId) throw new Error("Active Mac missing");
    await ctx.db.patch(tokenmaxxer.activeDeviceId, {
      generation: undefined,
      installationCredentialDigest: undefined,
      installationId: "legacy-unproved-installation",
    });
    return tokenmaxxer.activeDeviceId;
  });

  await t.run(async (ctx) => {
    await runToCompletion(
      ctx,
      components.migrations,
      internal.internal.migrations
        .retireLegacyActiveDeviceAuthority as MigrationFunctionReference,
    );
  });
  await expect(
    profile.authenticated.mutation(api.tokenmaxxers.ensureProfile, {
      displayName: "Fabien",
      expectedTouchGrassId: profile.touchGrassId,
      installationCredential: credential,
    }),
  ).resolves.toMatchObject({ activeMacGeneration: 1 });

  const authority = await t.run(async (ctx) => {
    const tokenmaxxer = await ctx.db
      .query("tokenmaxxers")
      .withIndex("by_public_id", (q) => q.eq("publicId", profile.touchGrassId))
      .unique();
    if (!tokenmaxxer?.activeDeviceId) throw new Error("replacement Active Mac missing");
    return {
      active: await ctx.db.get(tokenmaxxer.activeDeviceId),
      legacy: await ctx.db.get(legacyDeviceId),
    };
  });
  expect(authority.legacy?.installationId).toBeUndefined();
  expect(authority.legacy).toMatchObject({ revokedAt: expect.any(Number) });
  expect(authority.active).toMatchObject({
    generation: 1,
    installationCredentialDigest: expect.stringMatching(/^sha256:[0-9a-f]{64}$/),
  });
  expect(authority.active?._id).not.toBe(legacyDeviceId);
});

test("an unproved decrease rolls back the whole batch and an explicit correction commits", async () => {
  const t = testBackend();
  const credential = installationCredential("A");
  const { authenticated } = await createProfile(t, credential, "Fabien");
  await authenticated.mutation(api.sync.dailyUsage, {
    activeMacGeneration: 1,
    installationCredential: credential,
    snapshots: [usageSnapshot()],
  });
  const beforeRollback = await t.run(async (ctx) => ({
    publicScores: await ctx.db.query("publicScores").collect(),
    usageBuckets: await ctx.db.query("usageBuckets").collect(),
    userDailyUsage: await ctx.db.query("userDailyUsage").collect(),
    userScores: await ctx.db.query("userScores").collect(),
  }));

  await expect(
    authenticated.mutation(api.sync.dailyUsage, {
      activeMacGeneration: 1,
      installationCredential: credential,
      snapshots: [
        usageSnapshot({ observedTokens: 90, revision: 2 }),
        usageSnapshot({ provider: "claude" }),
      ],
    }),
  ).rejects.toThrow("correction provenance");
  const afterRollback = await t.run(async (ctx) => ({
    publicScores: await ctx.db.query("publicScores").collect(),
    usageBuckets: await ctx.db.query("usageBuckets").collect(),
    userDailyUsage: await ctx.db.query("userDailyUsage").collect(),
    userScores: await ctx.db.query("userScores").collect(),
  }));
  expect(afterRollback).toEqual(beforeRollback);

  await expect(
    authenticated.mutation(api.sync.dailyUsage, {
      activeMacGeneration: 1,
      installationCredential: credential,
      snapshots: [
        usageSnapshot({
          correctionReason: "parser-correction",
          correctionRevision: 2,
          observedTokens: 90,
          revision: 2,
        }),
      ],
    }),
  ).resolves.toMatchObject([{ outcome: "committed", revision: 2 }]);
});

test("a corrected revision can be the first server insert", async () => {
  const t = testBackend();
  const credential = installationCredential("A");
  const { authenticated } = await createProfile(t, credential, "Fabien");

  for (const snapshot of [
    usageSnapshot({
      correctionReason: "parser-correction",
      correctionRevision: 2,
      evidenceBasis: "provider-reported",
      revision: 2,
    }),
    usageSnapshot({
      correctionReason: "provider-replacement",
      correctionRevision: 2,
      revision: 2,
    }),
  ]) {
    await expect(
      authenticated.mutation(api.sync.dailyUsage, {
        activeMacGeneration: 1,
        installationCredential: credential,
        snapshots: [snapshot],
      }),
    ).rejects.toThrow("final evidence");
  }
  expect(await t.run(async (ctx) => ctx.db.query("usageBuckets").collect())).toEqual([]);

  await expect(
    authenticated.mutation(api.sync.dailyUsage, {
      activeMacGeneration: 1,
      installationCredential: credential,
      snapshots: [
        usageSnapshot({
          correctionReason: "parser-correction",
          correctionRevision: 2,
          observedTokens: 90,
          revision: 3,
        }),
        usageSnapshot({
          correctionReason: "provider-replacement",
          correctionRevision: 2,
          evidenceBasis: "provider-reported",
          observedTokens: 190,
          provider: "claude",
          revision: 3,
        }),
      ],
    }),
  ).resolves.toEqual([
    {
      outcome: "committed",
      provider: "codex",
      rankingDay: TODAY,
      revision: 3,
    },
    {
      outcome: "committed",
      provider: "claude",
      rankingDay: TODAY,
      revision: 3,
    },
  ]);

  const buckets = await t.run(async (ctx) => ctx.db.query("usageBuckets").collect());
  expect(
    buckets.map((bucket) => ({
      lastCorrectionReason: bucket.lastCorrectionReason,
      lastCorrectionRevision: bucket.lastCorrectionRevision,
      provider: bucket.provider,
      revision: bucket.revision,
    })),
  ).toEqual([
    {
      lastCorrectionReason: "parser-correction",
      lastCorrectionRevision: 2,
      provider: "codex",
      revision: 3,
    },
    {
      lastCorrectionReason: "provider-replacement",
      lastCorrectionRevision: 2,
      provider: "claude",
      revision: 3,
    },
  ]);
});

test("lost correction acknowledgements keep one lineage for both providers", async () => {
  const t = testBackend();
  const credential = installationCredential("A");
  const { authenticated } = await createProfile(t, credential, "Fabien");

  await authenticated.mutation(api.sync.dailyUsage, {
    activeMacGeneration: 1,
    installationCredential: credential,
    snapshots: [
      usageSnapshot(),
      usageSnapshot({ observedTokens: 200, provider: "claude" }),
    ],
  });
  await authenticated.mutation(api.sync.dailyUsage, {
    activeMacGeneration: 1,
    installationCredential: credential,
    snapshots: [
      usageSnapshot({
        correctionReason: "parser-correction",
        correctionRevision: 2,
        observedTokens: 90,
        revision: 2,
      }),
      usageSnapshot({
        correctionReason: "provider-replacement",
        correctionRevision: 2,
        evidenceBasis: "provider-reported",
        observedTokens: 190,
        provider: "claude",
        revision: 2,
      }),
    ],
  });

  await expect(
    authenticated.mutation(api.sync.dailyUsage, {
      activeMacGeneration: 1,
      installationCredential: credential,
      snapshots: [
        usageSnapshot({
          correctionReason: "parser-correction",
          correctionRevision: 2,
          observedTokens: 110,
          revision: 3,
        }),
        usageSnapshot({
          correctionReason: "provider-replacement",
          correctionRevision: 2,
          evidenceBasis: "provider-reported",
          observedTokens: 210,
          provider: "claude",
          revision: 3,
        }),
      ],
    }),
  ).resolves.toEqual([
    {
      outcome: "committed",
      provider: "codex",
      rankingDay: TODAY,
      revision: 3,
    },
    {
      outcome: "committed",
      provider: "claude",
      rankingDay: TODAY,
      revision: 3,
    },
  ]);

  for (const snapshot of [
    usageSnapshot({
      correctionReason: "parser-correction",
      correctionRevision: 2,
      observedTokens: 100,
      revision: 4,
    }),
    usageSnapshot({
      correctionReason: "provider-replacement",
      correctionRevision: 2,
      evidenceBasis: "provider-reported",
      observedTokens: 200,
      provider: "claude",
      revision: 4,
    }),
  ]) {
    await expect(
      authenticated.mutation(api.sync.dailyUsage, {
        activeMacGeneration: 1,
        installationCredential: credential,
        snapshots: [snapshot],
      }),
    ).rejects.toThrow("known correction lineage");
  }
  const stored = await t.run(async (ctx) => ({
    audits: await ctx.db.query("usageCorrectionAudits").collect(),
    buckets: await ctx.db.query("usageBuckets").collect(),
  }));
  expect(
    stored.audits.map(({ provider, reason, revision }) => ({
      provider,
      reason,
      revision,
    })),
  ).toEqual([
    { provider: "codex", reason: "parser-correction", revision: 2 },
    { provider: "claude", reason: "provider-replacement", revision: 2 },
  ]);
  expect(
    stored.buckets.map(
      ({ correctionReason, correctionRevision, observedTokens, provider, revision }) => ({
        correctionReason,
        correctionRevision,
        observedTokens,
        provider,
        revision,
      }),
    ),
  ).toEqual([
    {
      correctionReason: "parser-correction",
      correctionRevision: 2,
      observedTokens: 110,
      provider: "codex",
      revision: 3,
    },
    {
      correctionReason: "provider-replacement",
      correctionRevision: 2,
      observedTokens: 210,
      provider: "claude",
      revision: 3,
    },
  ]);
});

test("an acknowledged correction allows a later ordinary increase", async () => {
  const t = testBackend();
  const credential = installationCredential("A");
  const { authenticated } = await createProfile(t, credential, "Fabien");
  await authenticated.mutation(api.sync.dailyUsage, {
    activeMacGeneration: 1,
    installationCredential: credential,
    snapshots: [usageSnapshot()],
  });
  await authenticated.mutation(api.sync.dailyUsage, {
    activeMacGeneration: 1,
    installationCredential: credential,
    snapshots: [
      usageSnapshot({
        correctionReason: "parser-correction",
        correctionRevision: 2,
        observedTokens: 90,
        revision: 2,
      }),
    ],
  });

  await expect(
    authenticated.mutation(api.sync.dailyUsage, {
      activeMacGeneration: 1,
      installationCredential: credential,
      snapshots: [usageSnapshot({ observedTokens: 110, revision: 3 })],
    }),
  ).resolves.toMatchObject([{ outcome: "committed", revision: 3 }]);
  await expect(
    authenticated.mutation(api.sync.dailyUsage, {
      activeMacGeneration: 1,
      installationCredential: credential,
      snapshots: [usageSnapshot({ observedTokens: 100, revision: 4 })],
    }),
  ).rejects.toThrow("correction provenance");

  const stored = await t.run(async (ctx) => ({
    audits: await ctx.db.query("usageCorrectionAudits").collect(),
    buckets: await ctx.db.query("usageBuckets").collect(),
  }));
  expect(stored.audits).toHaveLength(1);
  expect(stored.audits[0]).toMatchObject({
    reason: "parser-correction",
    revision: 2,
  });
  expect(stored.buckets).toHaveLength(1);
  expect(stored.buckets[0]).toMatchObject({
    correctionReason: null,
    correctionRevision: null,
    lastCorrectionReason: "parser-correction",
    lastCorrectionRevision: 2,
    observedTokens: 110,
    revision: 3,
  });
});

test("a skipped correction revision is new after an older server revision", async () => {
  const t = testBackend();
  const credential = installationCredential("A");
  const { authenticated } = await createProfile(t, credential, "Fabien");
  await authenticated.mutation(api.sync.dailyUsage, {
    activeMacGeneration: 1,
    installationCredential: credential,
    snapshots: [
      usageSnapshot(),
      usageSnapshot({ observedTokens: 200, provider: "claude" }),
    ],
  });

  await expect(
    authenticated.mutation(api.sync.dailyUsage, {
      activeMacGeneration: 1,
      installationCredential: credential,
      snapshots: [
        usageSnapshot({
          correctionReason: "parser-correction",
          correctionRevision: 2,
          observedTokens: 90,
          revision: 3,
        }),
        usageSnapshot({
          correctionReason: "provider-replacement",
          correctionRevision: 2,
          evidenceBasis: "provider-reported",
          observedTokens: 190,
          provider: "claude",
          revision: 3,
        }),
      ],
    }),
  ).resolves.toMatchObject([
    { outcome: "committed", provider: "codex", revision: 3 },
    { outcome: "committed", provider: "claude", revision: 3 },
  ]);
  expect(
    await t.run(async (ctx) =>
      (await ctx.db.query("usageCorrectionAudits").collect()).map(
        ({ provider, revision }) => ({ provider, revision }),
      ),
    ),
  ).toEqual([
    { provider: "codex", revision: 2 },
    { provider: "claude", revision: 2 },
  ]);
});

test("correction reasons require exact evidence transitions", async () => {
  const t = testBackend();
  const credential = installationCredential("A");
  const { authenticated } = await createProfile(t, credential, "Fabien");
  await authenticated.mutation(api.sync.dailyUsage, {
    activeMacGeneration: 1,
    installationCredential: credential,
    snapshots: [usageSnapshot()],
  });

  for (const snapshot of [
    usageSnapshot({
      correctionReason: "provider-replacement",
      correctionRevision: 2,
      observedTokens: 90,
      revision: 2,
    }),
    usageSnapshot({
      correctionReason: "parser-correction",
      correctionRevision: 2,
      evidenceBasis: "provider-reported",
      observedTokens: 90,
      revision: 2,
    }),
  ]) {
    await expect(
      authenticated.mutation(api.sync.dailyUsage, {
        activeMacGeneration: 1,
        installationCredential: credential,
        snapshots: [snapshot],
      }),
    ).rejects.toThrow();
  }

  await expect(
    authenticated.mutation(api.sync.dailyUsage, {
      activeMacGeneration: 1,
      installationCredential: credential,
      snapshots: [
        usageSnapshot({
          correctionReason: "provider-replacement",
          correctionRevision: 2,
          evidenceBasis: "provider-reported",
          observedTokens: 90,
          revision: 2,
        }),
      ],
    }),
  ).resolves.toMatchObject([{ outcome: "committed", revision: 2 }]);

  for (const snapshot of [
    usageSnapshot({
      correctionReason: "provider-replacement",
      correctionRevision: 2,
      evidenceBasis: "provider-reported",
      observedTokens: 80,
      revision: 3,
    }),
    usageSnapshot({
      correctionReason: "parser-correction",
      correctionRevision: 2,
      observedTokens: 80,
      revision: 3,
    }),
  ]) {
    await expect(
      authenticated.mutation(api.sync.dailyUsage, {
        activeMacGeneration: 1,
        installationCredential: credential,
        snapshots: [snapshot],
      }),
    ).rejects.toThrow();
  }

  const buckets = await t.run(async (ctx) => ctx.db.query("usageBuckets").collect());
  expect(buckets).toHaveLength(1);
  expect(buckets[0]).toMatchObject({
    evidenceBasis: "provider-reported",
    observedTokens: 90,
    revision: 2,
  });
});

test("every accepted correction keeps a private append-only audit row", async () => {
  const t = testBackend();
  const credential = installationCredential("A");
  const { authenticated } = await createProfile(t, credential, "Fabien");
  for (const snapshot of [
    usageSnapshot(),
    usageSnapshot({
      correctionReason: "parser-correction",
      correctionRevision: 2,
      observedTokens: 90,
      revision: 2,
    }),
    usageSnapshot({
      correctionReason: "provider-replacement",
      correctionRevision: 3,
      evidenceBasis: "provider-reported",
      observedTokens: 80,
      revision: 3,
    }),
    usageSnapshot({
      correctionReason: "provider-replacement",
      correctionRevision: 3,
      evidenceBasis: "provider-reported",
      observedTokens: 95,
      revision: 4,
    }),
  ]) {
    await authenticated.mutation(api.sync.dailyUsage, {
      activeMacGeneration: 1,
      installationCredential: credential,
      snapshots: [snapshot],
    });
  }

  const stored = await t.run(async (ctx) => ({
    audits: await ctx.db.query("usageCorrectionAudits").collect(),
    buckets: await ctx.db.query("usageBuckets").collect(),
  }));
  expect(
    stored.audits.map(({ reason, revision }) => ({ reason, revision })),
  ).toEqual([
    { reason: "parser-correction", revision: 2 },
    { reason: "provider-replacement", revision: 3 },
  ]);
  expect(stored.buckets[0]).toMatchObject({
    correctionReason: "provider-replacement",
    correctionRevision: 3,
    lastCorrectionReason: "provider-replacement",
    lastCorrectionRevision: 3,
    revision: 4,
  });
});

test("hostile and non-canonical payloads fail before any usage write", async () => {
  const t = testBackend();
  const credential = installationCredential("A");
  const { authenticated } = await createProfile(t, credential, "Fabien");
  const baseArgs = {
    activeMacGeneration: 1,
    installationCredential: credential,
  };
  const invalidBatches: unknown[][] = (["codex", "claude"] as const).flatMap(
    (provider) => {
      const pricingBasis =
        provider === "codex"
          ? "openai-standard-2026-08-06-v1"
          : "anthropic-standard-2026-08-07-v1";
      return [
        Array.from({ length: 63 }, () => usageSnapshot({ provider })),
        [usageSnapshot({ provider }), usageSnapshot({ provider, revision: 2 })],
        [usageSnapshot({ provider, rankingDay: "2026-08-09" })],
        [
          usageSnapshot({
            observedAt: Date.parse("2026-08-07T23:59:59.999Z"),
            provider,
          }),
        ],
        [
          usageSnapshot({
            observedAt: NOW.getTime() + 5 * 60 * 1_000 + 1,
            provider,
          }),
        ],
        [usageSnapshot({ observedTokens: Number.MAX_SAFE_INTEGER + 1, provider })],
        [usageSnapshot({ correctionRevision: 1, provider })],
        [
          usageSnapshot({
            correctionReason: "parser-correction",
            correctionRevision: null,
            provider,
          }),
        ],
        [
          usageSnapshot({
            correctionReason: "parser-correction",
            correctionRevision: 0,
            provider,
            revision: 2,
          }),
        ],
        [
          usageSnapshot({
            correctionReason: "parser-correction",
            correctionRevision: 3,
            provider,
            revision: 2,
          }),
        ],
        [
          usageSnapshot({
            correctionReason: "parser-correction",
            correctionRevision: 1.5,
            provider,
            revision: 2,
          }),
        ],
        [
          usageSnapshot({
            apiEquivalentCost: {
              coveragePercent: null,
              micros: 1,
              pricingBasis,
              quality: "modeled",
            },
            provider,
          }),
        ],
        [
          usageSnapshot({
            apiEquivalentCost: {
              coveragePercent: null,
              micros: 1,
              pricingBasis,
              quality: "reconciled",
            },
            provider,
          }),
        ],
        [
          usageSnapshot({
            apiEquivalentCost: {
              coveragePercent: null,
              micros: 1,
              pricingBasis,
              quality: "local-only",
            },
            evidenceBasis: "provider-reported",
            provider,
          }),
        ],
        [
          usageSnapshot({
            apiEquivalentCost: {
              coveragePercent: null,
              micros: 1,
              pricingBasis: "provider-private-id",
              quality: "local-only",
            },
            provider,
          }),
        ],
        [
          {
            ...usageSnapshot({ provider }),
            privatePath: "/private/provider/session.jsonl",
          },
        ],
        [
          {
            ...usageSnapshot({ provider }),
            providerMessageId: "provider-private-id",
          },
        ],
      ];
    },
  );

  for (const snapshots of invalidBatches) {
    await expect(
      authenticated.mutation(api.sync.dailyUsage, {
        ...baseArgs,
        snapshots,
      } as never),
    ).rejects.toThrow();
  }
  await expect(
    authenticated.mutation(api.sync.dailyUsage, {
      ...baseArgs,
      installationCredential: installationCredential("A").slice(1),
      snapshots: [usageSnapshot()],
    }),
  ).rejects.toMatchObject({
    data: { code: "authority-rejected" },
    name: "ConvexError",
  });
  expect(
    await t.run(async (ctx) => ({
      publicScores: await ctx.db.query("publicScores").collect(),
      usageBuckets: await ctx.db.query("usageBuckets").collect(),
      userDailyUsage: await ctx.db.query("userDailyUsage").collect(),
      userScores: await ctx.db.query("userScores").collect(),
    })),
  ).toEqual({
    publicScores: [],
    usageBuckets: [],
    userDailyUsage: [],
    userScores: [],
  });
});

test("the app database stores only the installation digest and approved aggregate fields", async () => {
  const t = testBackend();
  const credential = installationCredential("A");
  const { authenticated } = await createProfile(t, credential, "Fabien");
  await authenticated.mutation(api.sync.dailyUsage, {
    activeMacGeneration: 1,
    installationCredential: credential,
    snapshots: [
      usageSnapshot(),
      usageSnapshot({
        correctionReason: "parser-correction",
        correctionRevision: 2,
        observedTokens: 200,
        provider: "claude",
        revision: 2,
      }),
    ],
  });

  const stored = await t.run(async (ctx) => ({
    devices: await ctx.db.query("devices").collect(),
    publicScores: await ctx.db.query("publicScores").collect(),
    tokenmaxxers: await ctx.db.query("tokenmaxxers").collect(),
    usageCorrectionAudits: await ctx.db.query("usageCorrectionAudits").collect(),
    usageBuckets: await ctx.db.query("usageBuckets").collect(),
    userDailyUsage: await ctx.db.query("userDailyUsage").collect(),
    userScores: await ctx.db.query("userScores").collect(),
  }));
  const serialized = JSON.stringify(stored);
  expect(serialized).not.toContain(credential);
  expect(serialized).not.toContain("privatePath");
  expect(serialized).not.toContain("providerMessageId");
  expect(stored.devices[0]?.installationCredentialDigest).toMatch(/^sha256:[0-9a-f]{64}$/);
  expect(stored.devices[0]?.installationId).toBeUndefined();
  expect(stored.usageCorrectionAudits).toHaveLength(1);
  expect(stored.usageCorrectionAudits[0]).toMatchObject({
    provider: "claude",
    rankingDay: TODAY,
    reason: "parser-correction",
    revision: 2,
  });
  expect(
    stored.usageBuckets.map(({ evidenceBasis, provider, rankingDay }) => ({
      evidenceBasis,
      provider,
      rankingDay,
    })),
  ).toEqual([
    {
      evidenceBasis: "locally-derived",
      provider: "codex",
      rankingDay: TODAY,
    },
    {
      evidenceBasis: "locally-derived",
      provider: "claude",
      rankingDay: TODAY,
    },
  ]);
});
