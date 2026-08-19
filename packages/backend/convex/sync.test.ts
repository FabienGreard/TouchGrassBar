/// <reference types="vite/client" />

import doomerboardIndexTest from "@convex-dev/aggregate/test";
import betterAuthTest from "@convex-dev/better-auth/test";
import migrationsTest from "@convex-dev/migrations/test";
import rateLimiterTest from "@convex-dev/rate-limiter/test";
import { convexTest } from "convex-test";
import { afterEach, beforeEach, expect, test, vi } from "vitest";

import { api, internal } from "./_generated/api";
import { createAuthWithRequestIp } from "./auth";
import {
  doomerboard,
  doomerboardKey,
  type DoomerboardKey,
} from "./model/doomerboard";
import { installationCredentialDigest } from "./model/profile";
import type { UsageSnapshot } from "./model/values";
import schema from "./schema";

const modules = import.meta.glob("./**/*.ts");
const TODAY = "2026-08-08";
const NOW = new Date(`${TODAY}T12:00:00.000Z`);
const TOUCH_GRASS_ID_ALPHABET = "23456789ABCDEFGHJKLMNPQRSTUVWXYZ";

function publicIdFor(index: number) {
  let remaining = index;
  let suffix = "";
  for (let position = 0; position < 6; position += 1) {
    suffix =
      TOUCH_GRASS_ID_ALPHABET[remaining % TOUCH_GRASS_ID_ALPHABET.length] +
      suffix;
    remaining = Math.floor(remaining / TOUCH_GRASS_ID_ALPHABET.length);
  }
  return `TG-${suffix}`;
}

function testBackend() {
  const t = convexTest(schema, modules);
  doomerboardIndexTest.register(t, "doomerboard");
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
    activeMacActivatedAt: NOW.getTime(),
    activeMacGeneration: 1,
    displayName,
    touchGrassId: profile.touchGrassId,
  });
  return profile;
}

async function transferActiveDevice(
  t: ReturnType<typeof testBackend>,
  touchGrassId: string,
  credential: string,
  generation: number,
) {
  const credentialDigest = await installationCredentialDigest(credential);
  return t.run(async (ctx) => {
    const tokenmaxxer = await ctx.db
      .query("tokenmaxxers")
      .withIndex("by_public_id", (q) => q.eq("publicId", touchGrassId))
      .unique();
    if (!tokenmaxxer?.activeDeviceId) {
      throw new Error("Active Mac missing");
    }
    await ctx.db.patch(tokenmaxxer.activeDeviceId, { revokedAt: Date.now() });
    const activeMacActivatedAt = Date.now();
    const deviceId = await ctx.db.insert("devices", {
      createdAt: activeMacActivatedAt,
      generation,
      installationCredentialDigest: credentialDigest,
      lastSeenAt: Date.now(),
      tokenmaxxerId: tokenmaxxer._id,
      usageBackfillCompletedAt: null,
    });
    await ctx.db.patch(tokenmaxxer._id, { activeDeviceId: deviceId });
    return activeMacActivatedAt;
  });
}

function usageSnapshot(overrides: Partial<UsageSnapshot> = {}): UsageSnapshot {
  const evidenceBasis = overrides.evidenceBasis ?? "locally-derived";
  const provider = overrides.provider ?? "codex";
  return {
    apiEquivalentCost: {
      coveragePercent: null,
      micros: 1_000,
      pricingBasis:
        provider === "codex"
          ? "openai-api-2026-08-09-v3"
          : "anthropic-standard-2026-08-07-v1",
      quality:
        evidenceBasis === "provider-reported" ? "reconciled" : "local-only",
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

test("first Profile accepts a thirty UTC day backfill atomically", async () => {
  const t = testBackend();
  const credential = installationCredential("A");
  const { authenticated } = await createProfile(t, credential, "Fabien");
  const snapshots = Array.from({ length: 30 }, (_, offset) => {
    const rankingDay = new Date(
      NOW.getTime() - offset * 24 * 60 * 60 * 1_000,
    )
      .toISOString()
      .slice(0, 10);
    return [
      usageSnapshot({
        evidenceBasis: "provider-reported",
        observedAt: NOW.getTime(),
        observedTokens: 100,
        provider: "codex",
        rankingDay,
      }),
      usageSnapshot({
        evidenceBasis: "locally-derived",
        observedAt: NOW.getTime(),
        observedTokens: 200,
        provider: "claude",
        rankingDay,
      }),
    ];
  }).flat();

  const first = await authenticated.mutation(api.sync.dailyUsage, {
    activeMacGeneration: 1,
    installationCredential: credential,
    profileBackfillAnchor: TODAY,
    snapshots,
  });
  expect(first).toHaveLength(60);
  expect(first.every(({ outcome }) => outcome === "committed")).toBe(true);

  const stored = await t.run(async (ctx) => ({
    buckets: await ctx.db.query("usageBuckets").collect(),
    daily: await ctx.db.query("userDailyUsage").collect(),
    device: await ctx.db.query("devices").first(),
    scores: await ctx.db.query("publicUsages").collect(),
  }));
  expect(stored.buckets).toHaveLength(60);
  expect(stored.daily).toHaveLength(60);
  expect(stored.device?.usageBackfillCompletedAt).toBe(NOW.getTime());
  expect(stored.scores).toContainEqual(
    expect.objectContaining({
      apiEquivalentCost: expect.objectContaining({ micros: 60_000 }),
      scope: "combined",
      tokenScore: 9_000,
      windowDays: 30,
    }),
  );

  const retry = await authenticated.mutation(api.sync.dailyUsage, {
    activeMacGeneration: 1,
    installationCredential: credential,
    profileBackfillAnchor: TODAY,
    snapshots,
  });
  expect(retry).toHaveLength(60);
  expect(retry.every(({ outcome }) => outcome === "idempotent")).toBe(true);
  expect(
    await t.run(async (ctx) => ctx.db.query("usageBuckets").collect()),
  ).toEqual(stored.buckets);

  const historicalCorrection = {
    ...snapshots.find(
      (snapshot) =>
        snapshot.provider === "claude" && snapshot.rankingDay !== TODAY,
    )!,
    correctionReason: "parser-correction" as const,
    correctionRevision: 2,
    observedAt: NOW.getTime() + 1_000,
    observedTokens: 180,
    revision: 2,
  };
  await expect(
    authenticated.mutation(api.sync.dailyUsage, {
      profileBackfillAnchor: null,
      activeMacGeneration: 1,
      installationCredential: credential,
      snapshots: [historicalCorrection],
    }),
  ).resolves.toEqual([
    {
      outcome: "committed",
      provider: "claude",
      rankingDay: historicalCorrection.rankingDay,
      revision: 2,
    },
  ]);
});

test("a completed sparse Profile backfill keeps missing days absent", async () => {
  const t = testBackend();
  const credential = installationCredential("A");
  const { authenticated } = await createProfile(t, credential, "Fabien");
  const oldestDay = new Date(NOW.getTime() - 29 * 24 * 60 * 60 * 1_000)
    .toISOString()
    .slice(0, 10);
  await authenticated.mutation(api.sync.dailyUsage, {
    activeMacGeneration: 1,
    installationCredential: credential,
    profileBackfillAnchor: TODAY,
    snapshots: [usageSnapshot(), usageSnapshot({ rankingDay: oldestDay })],
  });
  const missingDay = new Date(NOW.getTime() - 1 * 24 * 60 * 60 * 1_000)
    .toISOString()
    .slice(0, 10);

  await expect(
    authenticated.mutation(api.sync.dailyUsage, {
      profileBackfillAnchor: null,
      activeMacGeneration: 1,
      installationCredential: credential,
      snapshots: [usageSnapshot({ rankingDay: missingDay })],
    }),
  ).rejects.toThrow("keeps original-window missing days closed");
  expect(
    await t.run(async (ctx) => ctx.db.query("usageBuckets").collect()),
  ).toHaveLength(2);
});

test("an empty Profile backfill completes once and closes new historical days", async () => {
  const t = testBackend();
  const credential = installationCredential("A");
  const { authenticated } = await createProfile(t, credential, "Fabien");

  await expect(
    authenticated.mutation(api.sync.dailyUsage, {
      activeMacGeneration: 1,
      installationCredential: credential,
      profileBackfillAnchor: TODAY,
      snapshots: [],
    }),
  ).resolves.toEqual([]);
  expect(
    await t.run(async (ctx) =>
      (await ctx.db.query("devices").first())?.usageBackfillCompletedAt,
    ),
  ).toBe(NOW.getTime());

  await expect(
    authenticated.mutation(api.sync.dailyUsage, {
      activeMacGeneration: 1,
      installationCredential: credential,
      profileBackfillAnchor: TODAY,
      snapshots: [],
    }),
  ).resolves.toEqual([]);

  const previousDay = new Date(NOW.getTime() - 24 * 60 * 60 * 1_000)
    .toISOString()
    .slice(0, 10);
  await expect(
    authenticated.mutation(api.sync.dailyUsage, {
      profileBackfillAnchor: null,
      activeMacGeneration: 1,
      installationCredential: credential,
      snapshots: [usageSnapshot({ rankingDay: previousDay })],
    }),
  ).rejects.toThrow("keeps original-window missing days closed");
  await expect(
    authenticated.mutation(api.sync.dailyUsage, {
      profileBackfillAnchor: null,
      activeMacGeneration: 1,
      installationCredential: credential,
      snapshots: [usageSnapshot()],
    }),
  ).resolves.toMatchObject([{ outcome: "committed", revision: 1 }]);
});

test("a Profile backfill completion rolls back with one invalid row", async () => {
  const t = testBackend();
  const credential = installationCredential("A");
  const { authenticated } = await createProfile(t, credential, "Fabien");
  const outsideProfileWindow = new Date(
    NOW.getTime() - 30 * 24 * 60 * 60 * 1_000,
  )
    .toISOString()
    .slice(0, 10);

  await expect(
    authenticated.mutation(api.sync.dailyUsage, {
      activeMacGeneration: 1,
      installationCredential: credential,
      profileBackfillAnchor: TODAY,
      snapshots: [
        usageSnapshot(),
        usageSnapshot({ rankingDay: outsideProfileWindow }),
      ],
    }),
  ).rejects.toThrow("outside the Profile window");

  await expect(
    t.run(async (ctx) => ({
      buckets: await ctx.db.query("usageBuckets").collect(),
      completedAt: (await ctx.db.query("devices").first())
        ?.usageBackfillCompletedAt,
    })),
  ).resolves.toEqual({ buckets: [], completedAt: null });
});

test("a normal sync cannot use an empty batch or an invalid Profile anchor", async () => {
  const t = testBackend();
  const credential = installationCredential("A");
  const { authenticated } = await createProfile(t, credential, "Fabien");

  await expect(
    authenticated.mutation(api.sync.dailyUsage, {
      profileBackfillAnchor: null,
      activeMacGeneration: 1,
      installationCredential: credential,
      snapshots: [],
    }),
  ).rejects.toThrow("between 1 and 62");
  await expect(
    authenticated.mutation(api.sync.dailyUsage, {
      activeMacGeneration: 1,
      installationCredential: credential,
      profileBackfillAnchor: "2026-08-07",
      snapshots: [],
    }),
  ).rejects.toThrow("must match the Active Mac creation UTC day");
});

test("a marked Profile batch cannot join the next current UTC day", async () => {
  const t = testBackend();
  const credential = installationCredential("A");
  const { authenticated } = await createProfile(t, credential, "Fabien");
  const nextDay = "2026-08-09";
  const nextObservedAt = Date.parse(`${nextDay}T00:01:00.000Z`);
  vi.setSystemTime(nextObservedAt);
  const nextDaySnapshot = usageSnapshot({
    observedAt: nextObservedAt,
    rankingDay: nextDay,
  });

  await expect(
    authenticated.mutation(api.sync.dailyUsage, {
      activeMacGeneration: 1,
      installationCredential: credential,
      profileBackfillAnchor: TODAY,
      snapshots: [nextDaySnapshot],
    }),
  ).rejects.toThrow("marked Profile backfill snapshot is outside");
  await expect(
    authenticated.mutation(api.sync.dailyUsage, {
      activeMacGeneration: 1,
      installationCredential: credential,
      profileBackfillAnchor: null,
      snapshots: [nextDaySnapshot],
    }),
  ).resolves.toMatchObject([{ outcome: "committed", revision: 1 }]);
  await expect(
    authenticated.mutation(api.sync.dailyUsage, {
      activeMacGeneration: 1,
      installationCredential: credential,
      profileBackfillAnchor: TODAY,
      snapshots: [nextDaySnapshot],
    }),
  ).rejects.toThrow("marked Profile backfill snapshot is outside");
});

test("a marked Profile retry admits the latest anchor-day revision", async () => {
  const t = testBackend();
  const credential = installationCredential("A");
  const { authenticated } = await createProfile(t, credential, "Fabien");
  vi.setSystemTime(new Date("2026-08-09T00:01:00.000Z"));
  const latestAnchorSnapshot = usageSnapshot({
    observedAt: Date.parse(`${TODAY}T23:59:30.000Z`),
    observedTokens: 120,
    revision: 2,
  });
  const args = {
    activeMacGeneration: 1,
    installationCredential: credential,
    profileBackfillAnchor: TODAY,
    snapshots: [latestAnchorSnapshot],
  };

  await expect(
    authenticated.mutation(api.sync.dailyUsage, args),
  ).resolves.toEqual([
    {
      outcome: "committed",
      provider: "codex",
      rankingDay: TODAY,
      revision: 2,
    },
  ]);
  await expect(
    authenticated.mutation(api.sync.dailyUsage, args),
  ).resolves.toEqual([
    {
      outcome: "idempotent",
      provider: "codex",
      rankingDay: TODAY,
      revision: 2,
    },
  ]);
  expect(
    await t.run(async (ctx) => ctx.db.query("usageBuckets").first()),
  ).toMatchObject({ observedTokens: 120, revision: 2 });
});

test("a completed Profile accepts only in-day delayed post-anchor rows", async () => {
  const t = testBackend();
  const credential = installationCredential("A");
  const { authenticated } = await createProfile(t, credential, "Fabien");
  const secondRollover = Date.parse("2026-08-10T00:01:00.000Z");
  vi.setSystemTime(secondRollover);
  await authenticated.mutation(api.sync.dailyUsage, {
    activeMacGeneration: 1,
    installationCredential: credential,
    profileBackfillAnchor: TODAY,
    snapshots: [],
  });

  const delayedDay = "2026-08-09";
  const delayed = usageSnapshot({
    observedAt: Date.parse(`${delayedDay}T23:59:00.000Z`),
    observedTokens: 120,
    rankingDay: delayedDay,
    revision: 2,
  });
  const delayedArgs = {
    activeMacGeneration: 1,
    installationCredential: credential,
    profileBackfillAnchor: null,
    snapshots: [delayed],
  };
  await expect(
    authenticated.mutation(api.sync.dailyUsage, delayedArgs),
  ).resolves.toEqual([
    {
      outcome: "committed",
      provider: "codex",
      rankingDay: delayedDay,
      revision: 2,
    },
  ]);
  await expect(
    authenticated.mutation(api.sync.dailyUsage, delayedArgs),
  ).resolves.toEqual([
    {
      outcome: "idempotent",
      provider: "codex",
      rankingDay: delayedDay,
      revision: 2,
    },
  ]);

  await expect(
    authenticated.mutation(api.sync.dailyUsage, {
      activeMacGeneration: 1,
      installationCredential: credential,
      profileBackfillAnchor: null,
      snapshots: [usageSnapshot({ rankingDay: "2026-08-07" })],
    }),
  ).rejects.toThrow("keeps original-window missing days closed");
  await expect(
    authenticated.mutation(api.sync.dailyUsage, {
      activeMacGeneration: 1,
      installationCredential: credential,
      profileBackfillAnchor: null,
      snapshots: [
        usageSnapshot({
          observedAt: secondRollover,
          provider: "claude",
          rankingDay: delayedDay,
          revision: 2,
        }),
      ],
    }),
  ).rejects.toThrow("must use in-day observation evidence");
});

test("a marked Profile backfill rejects more than sixty total snapshots", async () => {
  const t = testBackend();
  const credential = installationCredential("A");
  const { authenticated } = await createProfile(t, credential, "Fabien");
  const snapshots = Array.from({ length: 31 }, (_, offset) => {
    const rankingDay = new Date(
      NOW.getTime() - offset * 24 * 60 * 60 * 1_000,
    )
      .toISOString()
      .slice(0, 10);
    return [
      usageSnapshot({ provider: "codex", rankingDay }),
      usageSnapshot({ provider: "claude", rankingDay }),
    ];
  })
    .flat()
    .slice(0, 61);

  await expect(
    authenticated.mutation(api.sync.dailyUsage, {
      activeMacGeneration: 1,
      installationCredential: credential,
      profileBackfillAnchor: TODAY,
      snapshots,
    }),
  ).rejects.toThrow("at most 60 Daily Usage Snapshots");
  expect(
    await t.run(async (ctx) => ctx.db.query("usageBuckets").collect()),
  ).toEqual([]);
});

test("a first Active Mac corrects a later current day after UTC rollover", async () => {
  const t = testBackend();
  const credential = installationCredential("A");
  const { authenticated } = await createProfile(t, credential, "Fabien");
  await authenticated.mutation(api.sync.dailyUsage, {
    profileBackfillAnchor: null,
    activeMacGeneration: 1,
    installationCredential: credential,
    snapshots: [usageSnapshot()],
  });

  const laterDay = "2026-08-09";
  const laterObservedAt = Date.parse(`${laterDay}T12:00:00.000Z`);
  vi.setSystemTime(laterObservedAt);
  await authenticated.mutation(api.sync.dailyUsage, {
    profileBackfillAnchor: null,
    activeMacGeneration: 1,
    installationCredential: credential,
    snapshots: [
      usageSnapshot({
        apiEquivalentCost: {
          coveragePercent: null,
          micros: 2_000,
          pricingBasis: "openai-api-2026-08-09-v3",
          quality: "local-only",
        },
        observedAt: laterObservedAt,
        observedTokens: 200,
        rankingDay: laterDay,
      }),
    ],
  });

  const correctionObservedAt = Date.parse("2026-08-10T12:00:00.000Z");
  vi.setSystemTime(correctionObservedAt);
  await expect(
    authenticated.mutation(api.sync.dailyUsage, {
      profileBackfillAnchor: null,
      activeMacGeneration: 1,
      installationCredential: credential,
      snapshots: [
        usageSnapshot({
          apiEquivalentCost: {
            coveragePercent: null,
            micros: 1_500,
            pricingBasis: "openai-api-2026-08-09-v3",
            quality: "local-only",
          },
          correctionReason: "parser-correction",
          correctionRevision: 2,
          observedAt: correctionObservedAt,
          observedTokens: 150,
          rankingDay: laterDay,
          revision: 2,
        }),
      ],
    }),
  ).resolves.toEqual([
    {
      outcome: "committed",
      provider: "codex",
      rankingDay: laterDay,
      revision: 2,
    },
  ]);

  const corrected = await t.run(async (ctx) => ({
    bucket: (
      await ctx.db.query("usageBuckets").collect()
    ).find((row) => row.rankingDay === laterDay),
    daily: (
      await ctx.db.query("userDailyUsage").collect()
    ).find((row) => row.rankingDay === laterDay),
    score: (
      await ctx.db.query("publicUsages").collect()
    ).find((row) => row.scope === "combined" && row.windowDays === 30),
  }));
  expect(corrected.bucket).toMatchObject({
    observedTokens: 150,
    revision: 2,
  });
  expect(corrected.daily).toMatchObject({ observedTokens: 150 });
  expect(corrected.score).toMatchObject({ tokenScore: 250 });
});

test("provider-reported evidence rejects higher and lower local replacements", async () => {
  const t = testBackend();
  const credential = installationCredential("A");
  const { authenticated } = await createProfile(t, credential, "Fabien");
  const providerSnapshot = usageSnapshot({
    evidenceBasis: "provider-reported",
    observedTokens: 100,
  });
  await authenticated.mutation(api.sync.dailyUsage, {
    profileBackfillAnchor: null,
    activeMacGeneration: 1,
    installationCredential: credential,
    snapshots: [providerSnapshot],
  });

  for (const observedTokens of [120, 80]) {
    await expect(
      authenticated.mutation(api.sync.dailyUsage, {
        profileBackfillAnchor: null,
        activeMacGeneration: 1,
        installationCredential: credential,
        snapshots: [
          usageSnapshot({
            observedAt: NOW.getTime() + observedTokens,
            observedTokens,
            revision: 2,
          }),
        ],
      }),
    ).resolves.toEqual([
      {
        outcome: "stale",
        provider: "codex",
        rankingDay: TODAY,
        revision: 2,
      },
    ]);
  }

  expect(
    await t.run(async (ctx) => ctx.db.query("usageBuckets").first()),
  ).toMatchObject({
    evidenceBasis: "provider-reported",
    observedTokens: 100,
    revision: 1,
  });
});

test("a retained Codex correction keeps an approved prior pricing basis", async () => {
  const t = testBackend();
  const credential = installationCredential("A");
  const { authenticated } = await createProfile(t, credential, "Fabien");
  const previousDay = new Date(NOW.getTime() - 24 * 60 * 60 * 1_000)
    .toISOString()
    .slice(0, 10);
  const priorCost = {
    coveragePercent: null,
    micros: 1_000,
    pricingBasis: "openai-standard-2026-08-06-v1",
    quality: "local-only" as const,
  };
  await authenticated.mutation(api.sync.dailyUsage, {
    activeMacGeneration: 1,
    installationCredential: credential,
    profileBackfillAnchor: TODAY,
    snapshots: [
      usageSnapshot({
        apiEquivalentCost: priorCost,
        rankingDay: previousDay,
      }),
    ],
  });

  await expect(
    authenticated.mutation(api.sync.dailyUsage, {
      profileBackfillAnchor: null,
      activeMacGeneration: 1,
      installationCredential: credential,
      snapshots: [
        usageSnapshot({
          apiEquivalentCost: priorCost,
          correctionReason: "parser-correction",
          correctionRevision: 2,
          observedAt: NOW.getTime() + 1_000,
          observedTokens: 90,
          rankingDay: previousDay,
          revision: 2,
        }),
      ],
    }),
  ).resolves.toMatchObject([{ outcome: "committed", revision: 2 }]);
  expect(
    await t.run(async (ctx) => ctx.db.query("usageBuckets").first()),
  ).toMatchObject({
    apiEquivalentCost: priorCost,
    observedTokens: 90,
    revision: 2,
  });
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
      profileBackfillAnchor: null,
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
  expect(
    await t.run(async (ctx) => ctx.db.query("publicUsages").collect()),
  ).toHaveLength(9);

  await expect(
    authenticated.mutation(api.sync.dailyUsage, {
      profileBackfillAnchor: null,
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
  expect(
    await t.run(async (ctx) => ctx.db.query("usageBuckets").collect()),
  ).toEqual(beforeRetry);

  const higherSnapshots = snapshots.map((snapshot) =>
    Object.assign({}, snapshot, {
      observedTokens: snapshot.observedTokens + 1,
    }),
  );
  await expect(
    authenticated.mutation(api.sync.dailyUsage, {
      profileBackfillAnchor: null,
      activeMacGeneration: 1,
      installationCredential: credential,
      snapshots: higherSnapshots,
    }),
  ).resolves.toEqual([
    {
      outcome: "stale",
      provider: "codex",
      rankingDay: TODAY,
      revision: 1,
    },
    {
      outcome: "stale",
      provider: "claude",
      rankingDay: TODAY,
      revision: 1,
    },
  ]);
  expect(
    await t.run(async (ctx) => ctx.db.query("usageBuckets").collect()),
  ).toEqual(beforeRetry);

  const lowerSnapshots = snapshots.map((snapshot) =>
    Object.assign({}, snapshot, {
      observedTokens: snapshot.observedTokens - 1,
    }),
  );
  await expect(
    authenticated.mutation(api.sync.dailyUsage, {
      profileBackfillAnchor: null,
      activeMacGeneration: 1,
      installationCredential: credential,
      snapshots: lowerSnapshots,
    }),
  ).resolves.toEqual([
    {
      outcome: "conflict",
      provider: "codex",
      rankingDay: TODAY,
      revision: 1,
    },
    {
      outcome: "conflict",
      provider: "claude",
      rankingDay: TODAY,
      revision: 1,
    },
  ]);
  expect(
    await t.run(async (ctx) => ctx.db.query("usageBuckets").collect()),
  ).toEqual(beforeRetry);

  const olderObservations = snapshots.map((snapshot) =>
    Object.assign({}, snapshot, {
      observedAt: snapshot.observedAt - 1,
      observedTokens: snapshot.observedTokens + 1,
      revision: 2,
    }),
  );
  await expect(
    authenticated.mutation(api.sync.dailyUsage, {
      profileBackfillAnchor: null,
      activeMacGeneration: 1,
      installationCredential: credential,
      snapshots: olderObservations,
    }),
  ).resolves.toEqual([
    {
      outcome: "conflict",
      provider: "codex",
      rankingDay: TODAY,
      revision: 2,
    },
    {
      outcome: "conflict",
      provider: "claude",
      rankingDay: TODAY,
      revision: 2,
    },
  ]);
  expect(
    await t.run(async (ctx) => ctx.db.query("usageBuckets").collect()),
  ).toEqual(beforeRetry);

  await authenticated.mutation(api.sync.dailyUsage, {
    profileBackfillAnchor: null,
    activeMacGeneration: 1,
    installationCredential: credential,
    snapshots: [usageSnapshot({ observedTokens: 110, revision: 3 })],
  });
  await expect(
    authenticated.mutation(api.sync.dailyUsage, {
      profileBackfillAnchor: null,
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

test("provider settings exclude accepted usage and restore retained history", async () => {
  const t = testBackend();
  const credential = installationCredential("A");
  const { authenticated } = await createProfile(t, credential, "Fabien");
  await authenticated.mutation(api.sync.dailyUsage, {
    profileBackfillAnchor: null,
    activeMacGeneration: 1,
    installationCredential: credential,
    snapshots: [
      usageSnapshot({ observedTokens: 100 }),
      usageSnapshot({ observedTokens: 200, provider: "claude" }),
    ],
  });

  await expect(
    authenticated.mutation(api.sync.providerSettings, {
      activeMacGeneration: 1,
      enabledProviders: ["codex"],
      installationCredential: credential,
      revision: 1,
    }),
  ).resolves.toEqual({ outcome: "committed", revision: 1 });
  await expect(
    authenticated.mutation(api.sync.providerSettings, {
      activeMacGeneration: 1,
      enabledProviders: ["codex"],
      installationCredential: credential,
      revision: 1,
    }),
  ).resolves.toEqual({ outcome: "idempotent", revision: 1 });
  await expect(
    authenticated.mutation(api.sync.providerSettings, {
      activeMacGeneration: 1,
      enabledProviders: ["codex", "claude"],
      installationCredential: credential,
      revision: 1,
    }),
  ).resolves.toEqual({ outcome: "stale", revision: 1 });

  const disabled = await t.run(async (ctx) => ({
    daily: await ctx.db.query("userDailyUsage").collect(),
    scores: await ctx.db.query("publicUsages").collect(),
    settings: await ctx.db.query("deviceProviderSettings").collect(),
  }));
  expect(disabled.daily).toHaveLength(2);
  expect(disabled.settings).toMatchObject([
    { claudeEnabled: false, codexEnabled: true, revision: 1 },
  ]);
  expect(
    disabled.scores.find(
      (row) => row.scope === "combined" && row.windowDays === 1,
    ),
  ).toMatchObject({ tokenScore: 100 });
  expect(
    disabled.scores.find(
      (row) => row.scope === "claude" && row.windowDays === 1,
    ),
  ).toMatchObject({ tokenScore: 0 });

  await expect(
    authenticated.mutation(api.sync.providerSettings, {
      activeMacGeneration: 1,
      enabledProviders: ["codex", "claude"],
      installationCredential: credential,
      revision: 2,
    }),
  ).resolves.toEqual({ outcome: "committed", revision: 2 });
  await expect(
    authenticated.mutation(api.sync.providerSettings, {
      activeMacGeneration: 1,
      enabledProviders: ["codex"],
      installationCredential: credential,
      revision: 1,
    }),
  ).resolves.toEqual({ outcome: "stale", revision: 2 });

  let combined = await t.run(async (ctx) =>
    (await ctx.db.query("publicUsages").collect()).find(
      (row) => row.scope === "combined" && row.windowDays === 1,
    ),
  );
  expect(combined).toMatchObject({ tokenScore: 300 });

  await authenticated.mutation(api.sync.providerSettings, {
    activeMacGeneration: 1,
    enabledProviders: [],
    installationCredential: credential,
    revision: 3,
  });
  combined = await t.run(async (ctx) =>
    (await ctx.db.query("publicUsages").collect()).find(
      (row) => row.scope === "combined" && row.windowDays === 1,
    ),
  );
  expect(combined).toMatchObject({ tokenScore: 0 });

  await authenticated.mutation(api.sync.providerSettings, {
    activeMacGeneration: 1,
    enabledProviders: ["claude"],
    installationCredential: credential,
    revision: 4,
  });
  combined = await t.run(async (ctx) =>
    (await ctx.db.query("publicUsages").collect()).find(
      (row) => row.scope === "combined" && row.windowDays === 1,
    ),
  );
  expect(combined).toMatchObject({ tokenScore: 200 });
});

test("provider settings require exact Active Mac authority and values", async () => {
  const t = testBackend();
  const credential = installationCredential("A");
  const { authenticated } = await createProfile(t, credential, "Fabien");

  await expect(
    authenticated.mutation(api.sync.providerSettings, {
      activeMacGeneration: 1,
      enabledProviders: ["codex"],
      installationCredential: installationCredential("B"),
      revision: 1,
    }),
  ).rejects.toThrow("authority-rejected");
  await expect(
    authenticated.mutation(api.sync.providerSettings, {
      activeMacGeneration: 1,
      enabledProviders: ["claude", "claude"],
      installationCredential: credential,
      revision: 1,
    }),
  ).rejects.toThrow("duplicate provider");
  expect(
    await t.run(async (ctx) =>
      ctx.db.query("deviceProviderSettings").collect(),
    ),
  ).toEqual([]);
});

test("modeled cost metadata reaches daily, score, and Doomerboard rows", async () => {
  const t = testBackend();
  const credential = installationCredential("A");
  const { authenticated } = await createProfile(t, credential, "Fabien");
  const apiEquivalentCost = {
    coveragePercent: 62.5,
    micros: 1_694_478,
    pricingBasis: "anthropic-standard-2026-08-07-v1",
    quality: "modeled" as const,
  };

  await authenticated.mutation(api.sync.dailyUsage, {
    profileBackfillAnchor: null,
    activeMacGeneration: 1,
    installationCredential: credential,
    snapshots: [
      usageSnapshot({
        apiEquivalentCost,
        observedTokens: 1_458_285,
        provider: "claude",
      }),
    ],
  });

  const stored = await t.run(async (ctx) => ({
    publicUsages: await ctx.db.query("publicUsages").collect(),
    userDailyUsage: await ctx.db.query("userDailyUsage").collect(),
  }));
  expect(stored.userDailyUsage).toHaveLength(1);
  expect(stored.userDailyUsage[0]).toMatchObject({
    apiEquivalentCost,
  });
  expect(
    stored.publicUsages.find(
      (row) => row.scope === "claude" && row.windowDays === 1,
    ),
  ).toMatchObject({
    apiEquivalentCost,
  });

  await expect(
    authenticated.query(api.doomerboards.global, {
      limit: 10,
      scope: "claude",
      windowDays: 1,
    }),
  ).resolves.toEqual([
    {
      apiEquivalentCost,
      displayName: "Fabien",
      rank: 1,
      tokenScore: 1_458_285,
      touchGrassId: expect.any(String),
    },
  ]);
});

test("Display Name changes immediately update Public Usage projections", async () => {
  const t = testBackend();
  const credential = installationCredential("A");
  const { authenticated } = await createProfile(t, credential, "Fabien");
  await authenticated.mutation(api.sync.dailyUsage, {
    profileBackfillAnchor: null,
    activeMacGeneration: 1,
    installationCredential: credential,
    snapshots: [usageSnapshot()],
  });

  await authenticated.mutation(api.tokenmaxxers.updateDisplayName, {
    displayName: "Updated Fabien",
  });

  const publicUsages = await t.run(async (ctx) =>
    ctx.db.query("publicUsages").collect(),
  );
  expect(publicUsages).toHaveLength(9);
  expect(publicUsages.every((row) => row.displayName === "Updated Fabien")).toBe(
    true,
  );
  await expect(
    authenticated.query(api.doomerboards.global, {
      limit: 10,
      scope: "combined",
      windowDays: 1,
    }),
  ).resolves.toMatchObject([{ displayName: "Updated Fabien" }]);
});

test("the current Global Doomerboard selects its provider and period", async () => {
  const t = testBackend();
  const credential = installationCredential("A");
  const { authenticated } = await createProfile(t, credential, "Fabien");
  await authenticated.mutation(api.sync.dailyUsage, {
    profileBackfillAnchor: null,
    activeMacGeneration: 1,
    installationCredential: credential,
    snapshots: [
      usageSnapshot({ observedTokens: 100, provider: "codex" }),
      usageSnapshot({ observedTokens: 250, provider: "claude" }),
    ],
  });

  await expect(
    authenticated.query(api.doomerboards.currentGlobal, {
      rankingDay: TODAY,
      scope: "claude",
      windowDays: 7,
    }),
  ).resolves.toMatchObject([{ tokenScore: 250 }]);
});

test("the current Friends Doomerboard selects its provider and period", async () => {
  const t = testBackend();
  const owner = await createProfile(t, installationCredential("A"), "Owner");
  const friendCredential = installationCredential("B");
  const friend = await createProfile(t, friendCredential, "Friend");
  await friend.authenticated.mutation(api.sync.dailyUsage, {
    profileBackfillAnchor: null,
    activeMacGeneration: 1,
    installationCredential: friendCredential,
    snapshots: [
      usageSnapshot({ observedTokens: 100, provider: "codex" }),
      usageSnapshot({ observedTokens: 250, provider: "claude" }),
    ],
  });
  await owner.authenticated.mutation(api.tokenmaxxers.addToMyTokenmaxxers, {
    touchGrassId: friend.touchGrassId,
  });

  await expect(
    owner.authenticated.query(api.doomerboards.currentMyTokenmaxxers, {
      rankingDay: TODAY,
      scope: "claude",
      windowDays: 30,
    }),
  ).resolves.toMatchObject({
    hasSavedTokenmaxxers: true,
    rows: [
      {
        displayName: "Friend",
        tokenScore: 250,
        touchGrassId: friend.touchGrassId,
      },
    ],
  });
});

test("My Tokenmaxxers rejects a new entry at its 100-entry limit", async () => {
  const t = testBackend();
  const owner = await createProfile(t, installationCredential("A"), "Owner");
  const targetTouchGrassId = publicIdFor(100);
  await t.run(async (ctx) => {
    const ownerProfile = await ctx.db
      .query("tokenmaxxers")
      .withIndex("by_public_id", (q) => q.eq("publicId", owner.touchGrassId))
      .unique();
    if (!ownerProfile) throw new Error("Owner Profile missing");
    for (let index = 0; index <= 100; index += 1) {
      const touchGrassId = publicIdFor(index);
      const addedId = await ctx.db.insert("tokenmaxxers", {
        authSubject: `saved-limit-${index}`,
        createdAt: NOW.getTime(),
        displayName: `Saved ${index}`,
        publicId: touchGrassId,
      });
      if (index < 100) {
        await ctx.db.insert("addedTokenmaxxers", {
          addedId,
          createdAt: NOW.getTime(),
          ownerId: ownerProfile._id,
        });
      }
    }
  });

  await expect(
    owner.authenticated.mutation(api.tokenmaxxers.addToMyTokenmaxxers, {
      touchGrassId: targetTouchGrassId,
    }),
  ).rejects.toThrow("My Tokenmaxxers limit reached");

  await t.run(async (ctx) => {
    const [ownerProfile, targetProfile] = await Promise.all([
      ctx.db
        .query("tokenmaxxers")
        .withIndex("by_public_id", (q) => q.eq("publicId", owner.touchGrassId))
        .unique(),
      ctx.db
        .query("tokenmaxxers")
        .withIndex("by_public_id", (q) => q.eq("publicId", targetTouchGrassId))
        .unique(),
    ]);
    if (!ownerProfile || !targetProfile) throw new Error("Profile missing");
    await ctx.db.insert("addedTokenmaxxers", {
      addedId: targetProfile._id,
      createdAt: NOW.getTime(),
      ownerId: ownerProfile._id,
    });
  });
  await expect(
    owner.authenticated.query(api.doomerboards.currentMyTokenmaxxers, {
      rankingDay: TODAY,
      scope: "combined",
      windowDays: 1,
    }),
  ).rejects.toThrow("My Tokenmaxxers limit exceeded");
  await expect(
    owner.authenticated.mutation(api.tokenmaxxers.removeFromMyTokenmaxxers, {
      touchGrassId: targetTouchGrassId,
    }),
  ).resolves.toBeNull();
  await expect(
    owner.authenticated.query(api.doomerboards.currentMyTokenmaxxers, {
      rankingDay: TODAY,
      scope: "combined",
      windowDays: 1,
    }),
  ).resolves.toEqual({ hasSavedTokenmaxxers: true, rows: [] });
});

test("the current Global Doomerboard is authenticated, current, bounded, deterministic, and public-only", async () => {
  const t = testBackend();
  const credential = installationCredential("A");
  const { authenticated } = await createProfile(t, credential, "Fabien");
  const expected = Array.from({ length: 105 }, (_, index) => ({
    displayName: `Tokenmaxxer ${index}`,
    tokenScore: 500_000 - Math.floor(index / 3),
    touchGrassId: publicIdFor(104 - index),
  })).sort(
    (left, right) =>
      right.tokenScore - left.tokenScore ||
      left.touchGrassId.localeCompare(right.touchGrassId),
  );
  const staleRows = Array.from({ length: 205 }, (_, index) => ({
    displayName: `Stale Tokenmaxxer ${index}`,
    tokenScore: 1_000_000 - index,
    touchGrassId: publicIdFor(309 - index),
  }));

  async function seedRows(
    rows: typeof expected,
    computedAt: number,
    keyFormat: "canonical" | "legacy" = "canonical",
  ) {
    for (let offset = 0; offset < rows.length; offset += 50) {
      await t.run(async (ctx) => {
        for (const row of rows.slice(offset, offset + 50)) {
          const tokenmaxxerId = await ctx.db.insert("tokenmaxxers", {
            authSubject: `seed-${row.touchGrassId}`,
            createdAt: NOW.getTime(),
            displayName: row.displayName,
            publicId: row.touchGrassId,
          });
          const publicUsageId = await ctx.db.insert("publicUsages", {
            apiEquivalentCost: null,
            boardKey: "tokens-v1:combined:1d",
            computedAt,
            displayName: row.displayName,
            scope: "combined",
            tokenmaxxerId,
            tokenScore: row.tokenScore,
            touchGrassId: row.touchGrassId,
            windowDays: 1,
          });
          await doomerboard.insert(ctx, {
            id: publicUsageId,
            key:
              keyFormat === "legacy"
                ? row.tokenScore
                : doomerboardKey(row.tokenScore, row.touchGrassId),
            namespace: "tokens-v1:combined:1d",
          });
        }
      });
    }
  }
  await seedRows(staleRows, NOW.getTime() - 24 * 60 * 60 * 1_000);
  await seedRows(expected.slice(0, 50), NOW.getTime(), "legacy");
  await seedRows(expected.slice(50), NOW.getTime());

  await expect(
    t.query(api.doomerboards.currentGlobal, {
      limit: 10,
      rankingDay: TODAY,
    }),
  ).rejects.toThrow("authority-rejected");
  await expect(
    authenticated.query(api.doomerboards.currentGlobal, {
      limit: 10,
      rankingDay: TODAY,
      tokenmaxxerId: "hostile-client-identity",
    } as never),
  ).rejects.toThrow();
  await expect(
    authenticated.query(api.doomerboards.currentGlobal, {
      rankingDay: "2026-02-30",
    }),
  ).rejects.toThrow("rankingDay is not a real UTC calendar day");

  const defaultPage = await authenticated.query(
    api.doomerboards.currentGlobal,
    { rankingDay: TODAY },
  );
  const cappedPage = await authenticated.query(
    api.doomerboards.currentGlobal,
    { limit: 500, rankingDay: TODAY },
  );
  expect(defaultPage).toHaveLength(50);
  expect(cappedPage).toHaveLength(100);
  expect(cappedPage).toEqual(
    expected.slice(0, 100).map((row, _index, rows) => ({
      apiEquivalentCost: null,
      displayName: row.displayName,
      rank:
        rows.findIndex((candidate) => candidate.tokenScore === row.tokenScore) +
        1,
      tokenScore: row.tokenScore,
      touchGrassId: row.touchGrassId,
    })),
  );
  expect(Object.keys(cappedPage[0] ?? {}).sort()).toEqual([
    "apiEquivalentCost",
    "displayName",
    "rank",
    "tokenScore",
    "touchGrassId",
  ]);
  expect(JSON.stringify(cappedPage)).not.toMatch(
    /authSubject|boardKey|computedAt|tokenmaxxerId|usageBucket|provider|path|prompt|session/,
  );
});

test("the legacy Global Doomerboard completes a score tie before ranking it", async () => {
  const t = testBackend();
  const credential = installationCredential("A");
  const { authenticated } = await createProfile(t, credential, "Fabien");
  const rows = Array.from({ length: 321 }, (_, index) => ({
    displayName: `Tied Tokenmaxxer ${index}`,
    touchGrassId: publicIdFor(index),
  }));

  for (let offset = 0; offset < rows.length; offset += 50) {
    await t.run(async (ctx) => {
      for (const row of rows.slice(offset, offset + 50)) {
        const tokenmaxxerId = await ctx.db.insert("tokenmaxxers", {
          authSubject: `legacy-tie-${row.touchGrassId}`,
          createdAt: NOW.getTime(),
          displayName: row.displayName,
          publicId: row.touchGrassId,
        });
        const publicUsageId = await ctx.db.insert("publicUsages", {
          apiEquivalentCost: null,
          boardKey: "tokens-v1:combined:1d",
          computedAt: NOW.getTime(),
          displayName: row.displayName,
          scope: "combined",
          tokenmaxxerId,
          tokenScore: 100,
          touchGrassId: row.touchGrassId,
          windowDays: 1,
        });
        await doomerboard.insert(ctx, {
          id: publicUsageId,
          key: 100,
          namespace: "tokens-v1:combined:1d",
        });
      }
    });
  }

  const page = await authenticated.query(api.doomerboards.currentGlobal, {
    limit: 100,
    rankingDay: TODAY,
  });
  expect(page.map((row) => row.touchGrassId)).toEqual(
    rows.slice(0, 100).map((row) => row.touchGrassId),
  );
  expect(page.every((row) => row.rank === 1)).toBe(true);
});

test("the compatibility read survives a missing index entry until migration repairs it", async () => {
  const t = testBackend();
  const credential = installationCredential("A");
  const { authenticated } = await createProfile(t, credential, "Fabien");
  await authenticated.mutation(api.sync.dailyUsage, {
    profileBackfillAnchor: null,
    activeMacGeneration: 1,
    installationCredential: credential,
    snapshots: [usageSnapshot()],
  });

  const publicUsage = await t.run(async (ctx) =>
    ctx.db
      .query("publicUsages")
      .withIndex("by_board_key", (q) => q.eq("boardKey", "tokens-v1:codex:1d"))
      .unique(),
  );
  if (!publicUsage) throw new Error("Public Usage missing");
  await t.run(async (ctx) => {
    await doomerboard.delete(ctx, {
      id: publicUsage._id,
      key: doomerboardKey(
        publicUsage.tokenScore,
        publicUsage.touchGrassId,
      ),
      namespace: publicUsage.boardKey,
    });
  });
  await expect(
    authenticated.query(api.doomerboards.global, {
      limit: 10,
      scope: "codex",
      windowDays: 1,
    }),
  ).resolves.toMatchObject([
    {
      displayName: "Fabien",
      rank: 1,
      tokenScore: 100,
    },
  ]);

  const args = { cursor: null, dryRun: false, oneBatchOnly: true };
  await t.mutation(
    internal.internal.migrations.backfillDoomerboard,
    args,
  );
  await t.mutation(
    internal.internal.migrations.backfillDoomerboard,
    args,
  );
  const repairedPage = await t.run((ctx) =>
    doomerboard.paginate(ctx, {
      bounds: {
        lower: {
          inclusive: true,
          key: [-Number.MAX_SAFE_INTEGER, ""] as DoomerboardKey,
        },
        upper: {
          inclusive: true,
          key: [0, "\uffff"] as DoomerboardKey,
        },
      },
      namespace: publicUsage.boardKey,
      order: "asc",
      pageSize: 10,
    }),
  );
  expect(repairedPage.page).toContainEqual({
    id: publicUsage._id,
    key: doomerboardKey(publicUsage.tokenScore, publicUsage.touchGrassId),
    sumValue: 0,
  });

  await expect(
    authenticated.query(api.doomerboards.global, {
      limit: 10,
      scope: "codex",
      windowDays: 1,
    }),
  ).resolves.toEqual([
    {
      apiEquivalentCost: expect.any(Object),
      displayName: "Fabien",
      rank: 1,
      tokenScore: 100,
      touchGrassId: expect.any(String),
    },
  ]);
});

test("the read-only Doomerboard invariant detects and verifies an idempotent repair", async () => {
  const t = testBackend();
  const credential = installationCredential("A");
  const { authenticated } = await createProfile(t, credential, "Fabien");
  await authenticated.mutation(api.sync.dailyUsage, {
    profileBackfillAnchor: null,
    activeMacGeneration: 1,
    installationCredential: credential,
    snapshots: [usageSnapshot()],
  });
  const publicUsage = await t.run(async (ctx) =>
    ctx.db
      .query("publicUsages")
      .withIndex("by_board_key", (q) =>
        q.eq("boardKey", "tokens-v1:combined:1d"),
      )
      .unique(),
  );
  if (!publicUsage) throw new Error("Public Usage missing");
  await t.run(async (ctx) => {
    await doomerboard.delete(ctx, {
      id: publicUsage._id,
      key: doomerboardKey(
        publicUsage.tokenScore,
        publicUsage.touchGrassId,
      ),
      namespace: publicUsage.boardKey,
    });
    await doomerboard.insert(ctx, {
      id: publicUsage._id,
      key: [
        -publicUsage.tokenScore + 1,
        publicUsage.touchGrassId,
      ],
      namespace: publicUsage.boardKey,
    });
    await doomerboard.insert(ctx, {
      id: publicUsage._id,
      key: doomerboardKey(
        publicUsage.tokenScore,
        publicUsage.touchGrassId,
      ),
      namespace: "invalid-board-key",
    });
  });

  await expect(
    t.action(internal.internal.doomerboardInvariant.check, {}),
  ).resolves.toMatchObject({
    extraEntries: 1,
    invalidEntries: 1,
    mismatchedEntries: 1,
    missingEntries: 1,
  });

  await expect(
    t.action(internal.internal.doomerboardInvariant.repair, {}),
  ).resolves.toEqual({ changedEntries: 2 });
  await expect(
    t.action(internal.internal.doomerboardInvariant.check, {}),
  ).resolves.toEqual({
    aggregateEntries: 9,
    extraEntries: 0,
    invalidEntries: 0,
    mismatchedEntries: 0,
    missingEntries: 0,
    publicScores: 9,
  });
  await expect(
    t.action(internal.internal.doomerboardInvariant.repair, {}),
  ).resolves.toEqual({ changedEntries: 0 });
  await expect(
    t.action(internal.internal.doomerboardInvariant.check, {}),
  ).resolves.toEqual({
    aggregateEntries: 9,
    extraEntries: 0,
    invalidEntries: 0,
    mismatchedEntries: 0,
    missingEntries: 0,
    publicScores: 9,
  });
});

test("the Doomerboard invariant repairs an invalid Public Usage namespace", async () => {
  const t = testBackend();
  const credential = installationCredential("A");
  const { authenticated } = await createProfile(t, credential, "Fabien");
  await authenticated.mutation(api.sync.dailyUsage, {
    profileBackfillAnchor: null,
    activeMacGeneration: 1,
    installationCredential: credential,
    snapshots: [usageSnapshot()],
  });
  const publicUsage = await t.run(async (ctx) =>
    ctx.db
      .query("publicUsages")
      .withIndex("by_board_key", (q) =>
        q.eq("boardKey", "tokens-v1:combined:1d"),
      )
      .unique(),
  );
  if (!publicUsage) throw new Error("Public Usage missing");
  await t.run(async (ctx) => {
    await ctx.db.patch(publicUsage._id, { boardKey: "invalid-board-key" });
  });

  await expect(
    t.action(internal.internal.doomerboardInvariant.check, {}),
  ).resolves.toMatchObject({
    extraEntries: 1,
    invalidEntries: 1,
    missingEntries: 1,
  });
  await expect(
    t.action(internal.internal.doomerboardInvariant.repair, {}),
  ).resolves.toEqual({ changedEntries: 2 });
  await expect(
    t.action(internal.internal.doomerboardInvariant.check, {}),
  ).resolves.toEqual({
    aggregateEntries: 9,
    extraEntries: 0,
    invalidEntries: 0,
    mismatchedEntries: 0,
    missingEntries: 0,
    publicScores: 9,
  });
  await expect(
    t.action(internal.internal.doomerboardInvariant.repair, {}),
  ).resolves.toEqual({ changedEntries: 0 });
});

test("the device completion migration preserves pending Profile authority", async () => {
  const t = testBackend();
  const credential = installationCredential("A");
  const { authenticated } = await createProfile(t, credential, "Fabien");
  await t.run(async (ctx) => {
    const device = await ctx.db.query("devices").unique();
    if (!device) throw new Error("Active Mac missing");
    const {
      _creationTime,
      _id,
      usageBackfillCompletedAt: _usageBackfillCompletedAt,
      ...legacyDevice
    } = device;
    expect(_creationTime).toBeGreaterThan(0);
    await ctx.db.replace(_id, legacyDevice);
  });

  await expect(
    authenticated.mutation(api.sync.dailyUsage, {
      profileBackfillAnchor: null,
      activeMacGeneration: 1,
      installationCredential: credential,
      snapshots: [
        usageSnapshot({
          rankingDay: "2026-08-07",
          observedAt: Date.parse("2026-08-07T12:00:00.000Z"),
        }),
      ],
    }),
  ).rejects.toThrow(
    "new Profile history requires an explicit backfill completion marker",
  );

  const args = { cursor: null, dryRun: false, oneBatchOnly: true };
  await t.mutation(
    internal.internal.migrations.backfillDeviceUsageCompletion,
    args,
  );
  await t.mutation(
    internal.internal.migrations.backfillDeviceUsageCompletion,
    args,
  );
  expect(
    await t.run(async (ctx) =>
      (await ctx.db.query("devices").unique())?.usageBackfillCompletedAt,
    ),
  ).toBeNull();

  await expect(
    authenticated.mutation(api.sync.dailyUsage, {
      profileBackfillAnchor: TODAY,
      activeMacGeneration: 1,
      installationCredential: credential,
      snapshots: [],
    }),
  ).resolves.toEqual([]);
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
      profileBackfillAnchor: null,
      activeMacGeneration: 1,
      installationCredential: bobCredential,
      snapshots: [snapshot],
    }),
    alice.authenticated.mutation(api.sync.dailyUsage, {
      profileBackfillAnchor: null,
      activeMacGeneration: 2,
      installationCredential: aliceCredential,
      snapshots: [snapshot],
    }),
    bob.authenticated.mutation(api.sync.dailyUsage, {
      profileBackfillAnchor: null,
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
      profileBackfillAnchor: null,
      activeMacGeneration: 1,
      installationCredential: aliceCredential,
      snapshots: [snapshot],
    }),
  ).rejects.toThrow("authority-rejected");
  expect(
    await t.run(async (ctx) => ctx.db.query("usageBuckets").collect()),
  ).toEqual([]);
});

test("same-day Active Mac transfer freezes and adds both provider segments", async () => {
  const t = testBackend();
  const oldCredential = installationCredential("A");
  const newCredential = installationCredential("B");
  const profile = await createProfile(t, oldCredential, "Fabien");

  await profile.authenticated.mutation(api.sync.dailyUsage, {
    profileBackfillAnchor: null,
    activeMacGeneration: 1,
    installationCredential: oldCredential,
    snapshots: [
      usageSnapshot({ observedTokens: 100 }),
      usageSnapshot({
        apiEquivalentCost: {
          coveragePercent: null,
          micros: 2_000,
          pricingBasis: "anthropic-standard-2026-08-07-v1",
          quality: "reconciled",
        },
        evidenceBasis: "provider-reported",
        observedTokens: 200,
        provider: "claude",
      }),
    ],
  });
  const activeMacActivatedAt = await transferActiveDevice(
    t,
    profile.touchGrassId,
    newCredential,
    2,
  );
  await expect(
    profile.authenticated.mutation(api.tokenmaxxers.ensureProfile, {
      displayName: "Fabien",
      expectedTouchGrassId: profile.touchGrassId,
      installationCredential: newCredential,
    }),
  ).resolves.toEqual({
    activeMacActivatedAt,
    activeMacGeneration: 2,
    displayName: "Fabien",
    touchGrassId: profile.touchGrassId,
  });

  await expect(
    profile.authenticated.mutation(api.sync.dailyUsage, {
      profileBackfillAnchor: null,
      activeMacGeneration: 1,
      installationCredential: oldCredential,
      snapshots: [usageSnapshot({ observedTokens: 150, revision: 2 })],
    }),
  ).rejects.toThrow("authority-rejected");

  await profile.authenticated.mutation(api.sync.dailyUsage, {
    profileBackfillAnchor: null,
    activeMacGeneration: 2,
    installationCredential: newCredential,
    snapshots: [
      usageSnapshot({
        apiEquivalentCost: {
          coveragePercent: 50,
          micros: 500,
          pricingBasis: "openai-api-2026-08-09-v3",
          quality: "modeled",
        },
        observedTokens: 50,
      }),
      usageSnapshot({
        apiEquivalentCost: null,
        observedTokens: 75,
        provider: "claude",
      }),
    ],
  });

  const stored = await t.run(async (ctx) => ({
    publicUsages: await ctx.db.query("publicUsages").collect(),
    usageBuckets: await ctx.db.query("usageBuckets").collect(),
    userDailyUsage: await ctx.db.query("userDailyUsage").collect(),
  }));
  expect(stored.usageBuckets).toHaveLength(4);
  expect(
    stored.usageBuckets
      .filter((row) => row.provider === "codex")
      .map((row) => row.observedTokens)
      .sort((left, right) => left - right),
  ).toEqual([50, 100]);
  expect(
    stored.usageBuckets
      .filter((row) => row.provider === "claude")
      .map((row) => row.observedTokens)
      .sort((left, right) => left - right),
  ).toEqual([75, 200]);
  expect(stored.userDailyUsage).toHaveLength(2);

  const codexDaily = stored.userDailyUsage.find(
    (row) => row.provider === "codex",
  );
  const claudeDaily = stored.userDailyUsage.find(
    (row) => row.provider === "claude",
  );
  expect(codexDaily).toMatchObject({
    apiEquivalentCost: {
      micros: 1_500,
      pricingBasis: "openai-api-2026-08-09-v3",
      quality: "modeled",
    },
    observedTokens: 150,
  });
  expect(codexDaily?.apiEquivalentCost?.coveragePercent).toBeCloseTo(
    83.333_333,
  );
  expect(claudeDaily).toMatchObject({
    apiEquivalentCost: {
      micros: 2_000,
      pricingBasis: "anthropic-standard-2026-08-07-v1",
      quality: "modeled",
    },
    observedTokens: 275,
  });
  expect(claudeDaily?.apiEquivalentCost?.coveragePercent).toBeCloseTo(
    72.727_273,
  );

  const combinedUsage = stored.publicUsages.find(
    (row) => row.scope === "combined" && row.windowDays === 1,
  );
  expect(combinedUsage).toMatchObject({
    apiEquivalentCost: {
      micros: 3_500,
      pricingBasis:
        "anthropic-standard-2026-08-07-v1 + openai-api-2026-08-09-v3",
      quality: "modeled",
    },
    tokenScore: 425,
  });
  expect(combinedUsage?.apiEquivalentCost?.coveragePercent).toBeCloseTo(
    76.470_588,
  );
});

test("a rollover accepts a zero transfer carryover after activation", async () => {
  const t = testBackend();
  const oldCredential = installationCredential("A");
  const newCredential = installationCredential("B");
  const profile = await createProfile(t, oldCredential, "Fabien");

  const transferTime = new Date(`${TODAY}T23:59:00.000Z`);
  vi.setSystemTime(transferTime);
  await profile.authenticated.mutation(api.sync.dailyUsage, {
    profileBackfillAnchor: null,
    activeMacGeneration: 1,
    installationCredential: oldCredential,
    snapshots: [
      usageSnapshot({
        observedAt: transferTime.getTime() - 60_000,
        observedTokens: 100,
      }),
    ],
  });
  const activeMacActivatedAt = await transferActiveDevice(
    t,
    profile.touchGrassId,
    newCredential,
    2,
  );

  vi.setSystemTime(new Date("2026-08-09T00:01:00.000Z"));
  const marker = usageSnapshot({
    apiEquivalentCost: null,
    coverage: "partial",
    observedAt: activeMacActivatedAt + 30_000,
    observedTokens: 0,
    rankingDay: TODAY,
  });
  await expect(
    profile.authenticated.mutation(api.sync.dailyUsage, {
      profileBackfillAnchor: null,
      activeMacGeneration: 1,
      installationCredential: oldCredential,
      snapshots: [marker],
    }),
  ).rejects.toThrow("authority-rejected");
  await expect(
    profile.authenticated.mutation(api.sync.dailyUsage, {
      profileBackfillAnchor: null,
      activeMacGeneration: 2,
      installationCredential: newCredential,
      snapshots: [marker],
    }),
  ).resolves.toEqual([
    {
      outcome: "committed",
      provider: "codex",
      rankingDay: TODAY,
      revision: 1,
    },
  ]);
  await expect(
    profile.authenticated.mutation(api.sync.dailyUsage, {
      profileBackfillAnchor: null,
      activeMacGeneration: 2,
      installationCredential: newCredential,
      snapshots: [marker],
    }),
  ).resolves.toEqual([
    {
      outcome: "idempotent",
      provider: "codex",
      rankingDay: TODAY,
      revision: 1,
    },
  ]);

  const stored = await t.run(async (ctx) => ({
    publicUsages: await ctx.db.query("publicUsages").collect(),
    usageBuckets: await ctx.db.query("usageBuckets").collect(),
    userDailyUsage: await ctx.db.query("userDailyUsage").collect(),
  }));
  expect(stored.usageBuckets).toHaveLength(2);
  expect(stored.usageBuckets).toContainEqual(
    expect.objectContaining({
      apiEquivalentCost: expect.objectContaining({ micros: 1_000 }),
      observedTokens: 100,
      provider: "codex",
      rankingDay: TODAY,
    }),
  );
  expect(stored.usageBuckets).toContainEqual(
    expect.objectContaining({
      apiEquivalentCost: null,
      coverage: "partial",
      observedAt: activeMacActivatedAt + 30_000,
      observedTokens: 0,
      provider: "codex",
      rankingDay: TODAY,
      revision: 1,
    }),
  );
  expect(stored.userDailyUsage).toContainEqual(
    expect.objectContaining({
      apiEquivalentCost: null,
      observedTokens: 100,
      provider: "codex",
      rankingDay: TODAY,
    }),
  );
  const affectedPublicUsages = stored.publicUsages.filter(
    (usage) => usage.tokenScore === 100,
  );
  expect(affectedPublicUsages).toHaveLength(4);
  expect(
    affectedPublicUsages.every((usage) => usage.apiEquivalentCost === null),
  ).toBe(true);

  const currentDay = "2026-08-09";
  await expect(
    profile.authenticated.mutation(api.sync.dailyUsage, {
      profileBackfillAnchor: null,
      activeMacGeneration: 2,
      installationCredential: newCredential,
      snapshots: [
        usageSnapshot({
          observedAt: Date.now(),
          observedTokens: 25,
          rankingDay: currentDay,
        }),
      ],
    }),
  ).resolves.toEqual([
    {
      outcome: "committed",
      provider: "codex",
      rankingDay: currentDay,
      revision: 1,
    },
  ]);
});

test("a rollover commits and retries a partial transfer-day segment", async () => {
  const t = testBackend();
  const oldCredential = installationCredential("A");
  const newCredential = installationCredential("B");
  const profile = await createProfile(t, oldCredential, "Fabien");

  const acceptedTime = new Date(`${TODAY}T23:57:00.000Z`);
  vi.setSystemTime(acceptedTime);
  await profile.authenticated.mutation(api.sync.dailyUsage, {
    profileBackfillAnchor: null,
    activeMacGeneration: 1,
    installationCredential: oldCredential,
    snapshots: [
      usageSnapshot({
        observedAt: acceptedTime.getTime(),
        observedTokens: 100,
      }),
    ],
  });
  const transferTime = new Date(`${TODAY}T23:58:00.000Z`);
  vi.setSystemTime(transferTime);
  const activeMacActivatedAt = await transferActiveDevice(
    t,
    profile.touchGrassId,
    newCredential,
    2,
  );
  const segment = usageSnapshot({
    coverage: "partial",
    observedAt: activeMacActivatedAt + 60_000,
    observedTokens: 50,
    rankingDay: TODAY,
    revision: 3,
  });

  vi.setSystemTime(new Date("2026-08-09T00:01:00.000Z"));
  await expect(
    profile.authenticated.mutation(api.sync.dailyUsage, {
      profileBackfillAnchor: null,
      activeMacGeneration: 2,
      installationCredential: newCredential,
      snapshots: [segment],
    }),
  ).resolves.toEqual([
    {
      outcome: "committed",
      provider: "codex",
      rankingDay: TODAY,
      revision: 3,
    },
  ]);
  await expect(
    profile.authenticated.mutation(api.sync.dailyUsage, {
      profileBackfillAnchor: null,
      activeMacGeneration: 2,
      installationCredential: newCredential,
      snapshots: [segment],
    }),
  ).resolves.toEqual([
    {
      outcome: "idempotent",
      provider: "codex",
      rankingDay: TODAY,
      revision: 3,
    },
  ]);

  const stored = await t.run(async (ctx) => ({
    publicUsages: await ctx.db.query("publicUsages").collect(),
    usageBuckets: await ctx.db.query("usageBuckets").collect(),
    userDailyUsage: await ctx.db.query("userDailyUsage").collect(),
  }));
  expect(stored.usageBuckets).toContainEqual(
    expect.objectContaining({
      apiEquivalentCost: expect.objectContaining({ micros: 1_000 }),
      coverage: "partial",
      observedAt: activeMacActivatedAt + 60_000,
      observedTokens: 50,
      provider: "codex",
      rankingDay: TODAY,
      revision: 3,
    }),
  );
  expect(stored.userDailyUsage).toContainEqual(
    expect.objectContaining({
      apiEquivalentCost: null,
      observedTokens: 150,
      provider: "codex",
      rankingDay: TODAY,
    }),
  );
  const affectedPublicUsages = stored.publicUsages.filter(
    (usage) => usage.tokenScore === 150,
  );
  expect(affectedPublicUsages).toHaveLength(4);
  expect(
    affectedPublicUsages.every((usage) => usage.apiEquivalentCost === null),
  ).toBe(true);
});

test("the first Active Mac can backfill its creation day after UTC rollover", async () => {
  const t = testBackend();
  const credential = installationCredential("A");
  const profile = await createProfile(t, credential, "Fabien");
  vi.setSystemTime(new Date("2026-08-09T00:01:00.000Z"));

  await expect(
    profile.authenticated.mutation(api.sync.dailyUsage, {
      activeMacGeneration: 1,
      installationCredential: credential,
      profileBackfillAnchor: TODAY,
      snapshots: [
        usageSnapshot({
          apiEquivalentCost: null,
          coverage: "partial",
          observedAt: NOW.getTime(),
          observedTokens: 0,
        }),
      ],
    }),
  ).resolves.toEqual([
    {
      outcome: "committed",
      provider: "codex",
      rankingDay: TODAY,
      revision: 1,
    },
  ]);
  expect(
    await t.run(async (ctx) => ctx.db.query("usageBuckets").collect()),
  ).toHaveLength(1);
});

test("historical usage rejects every value outside a transfer carryover", async () => {
  const t = testBackend();
  const oldCredential = installationCredential("A");
  const newCredential = installationCredential("B");
  const profile = await createProfile(t, oldCredential, "Fabien");
  const transferTime = new Date(`${TODAY}T23:59:00.000Z`);
  vi.setSystemTime(transferTime);
  const activeMacActivatedAt = await transferActiveDevice(
    t,
    profile.touchGrassId,
    newCredential,
    2,
  );
  vi.setSystemTime(new Date("2026-08-09T00:01:00.000Z"));

  const validMarker = usageSnapshot({
    apiEquivalentCost: null,
    coverage: "partial",
    observedAt: activeMacActivatedAt,
    observedTokens: 0,
    rankingDay: TODAY,
  });
  const validSegment = usageSnapshot({
    coverage: "partial",
    observedAt: activeMacActivatedAt + 1,
    observedTokens: 1,
    rankingDay: TODAY,
    revision: 2,
  });
  const hostileSnapshots: UsageSnapshot[] = [
    { ...validSegment, rankingDay: "2026-08-07" },
    { ...validSegment, observedAt: activeMacActivatedAt - 1 },
    { ...validSegment, coverage: "complete" },
    {
      ...validMarker,
      apiEquivalentCost: {
        coveragePercent: null,
        micros: 1,
        pricingBasis: "openai-api-2026-08-09-v3",
        quality: "local-only",
      },
    },
    {
      ...validMarker,
      correctionReason: "parser-correction",
      correctionRevision: 1,
    },
    { ...validMarker, revision: 2 },
  ];

  for (const snapshot of hostileSnapshots) {
    await expect(
      profile.authenticated.mutation(api.sync.dailyUsage, {
        profileBackfillAnchor: null,
        activeMacGeneration: 2,
        installationCredential: newCredential,
        snapshots: [snapshot],
      }),
    ).rejects.toThrow("transfer");
  }
  expect(
    await t.run(async (ctx) => ctx.db.query("usageBuckets").collect()),
  ).toEqual([]);
});

test("score recomputation ignores more than 1000 old rows", async () => {
  const t = testBackend();
  const credential = installationCredential("A");
  const profile = await createProfile(t, credential, "Fabien");
  const tokenmaxxerId = await t.run(async (ctx) => {
    const tokenmaxxer = await ctx.db
      .query("tokenmaxxers")
      .withIndex("by_public_id", (q) => q.eq("publicId", profile.touchGrassId))
      .unique();
    if (!tokenmaxxer) throw new Error("Tokenmaxxer missing");
    return tokenmaxxer._id;
  });
  const oldRows = Array.from({ length: 501 }, (_, dayOffset) =>
    (["codex", "claude"] as const).map((provider) => ({
      apiEquivalentCost: null,
      observedTokens: 10_000 + dayOffset,
      provider,
      rankingDay: new Date(Date.UTC(2020, 0, dayOffset + 1))
        .toISOString()
        .slice(0, 10),
      tokenmaxxerId,
      updatedAt: NOW.getTime(),
    })),
  ).flat();
  for (let offset = 0; offset < oldRows.length; offset += 200) {
    const rows = oldRows.slice(offset, offset + 200);
    await t.run(async (ctx) => {
      for (const row of rows) {
        await ctx.db.insert("userDailyUsage", row);
      }
    });
  }
  await t.run(async (ctx) => {
    await ctx.db.insert("userDailyUsage", {
      apiEquivalentCost: null,
      observedTokens: 101,
      provider: "codex",
      rankingDay: TODAY,
      tokenmaxxerId,
      updatedAt: NOW.getTime(),
    });
    await ctx.db.insert("userDailyUsage", {
      apiEquivalentCost: null,
      observedTokens: 202,
      provider: "claude",
      rankingDay: TODAY,
      tokenmaxxerId,
      updatedAt: NOW.getTime(),
    });
  });

  await t.mutation(internal.internal.recompute.one, { tokenmaxxerId });

  const scores = await t.run(async (ctx) =>
    ctx.db
      .query("publicUsages")
      .withIndex("by_tokenmaxxer_id", (q) =>
        q.eq("tokenmaxxerId", tokenmaxxerId),
      )
      .take(9),
  );
  expect(scores).toHaveLength(9);
  for (const windowDays of [1, 7, 30] as const) {
    expect(
      scores.find(
        (row) => row.scope === "codex" && row.windowDays === windowDays,
      ),
    ).toMatchObject({ tokenScore: 101 });
    expect(
      scores.find(
        (row) => row.scope === "claude" && row.windowDays === windowDays,
      ),
    ).toMatchObject({ tokenScore: 202 });
    expect(
      scores.find(
        (row) => row.scope === "combined" && row.windowDays === windowDays,
      ),
    ).toMatchObject({ tokenScore: 303 });
  }
});

test("daily score recomputation drains every Tokenmaxxer", async () => {
  const t = testBackend();
  const tokenmaxxerIds = await t.run(async (ctx) => {
    const ids = [];
    for (let index = 0; index < 202; index += 1) {
      ids.push(
        await ctx.db.insert("tokenmaxxers", {
          authSubject: `rollover-${index}`,
          createdAt: NOW.getTime(),
          displayName: `Tokenmaxxer ${index}`,
          ...(index === 200
            ? { lastSyncedAt: NOW.getTime() - 46 * 24 * 60 * 60 * 1_000 }
            : index === 201
              ? {}
              : { lastSyncedAt: NOW.getTime() }),
          publicId: `TG-${String(index).padStart(6, "0")}`,
        }),
      );
    }
    return ids;
  });

  await t.mutation(internal.internal.recompute.scheduleAll, {});
  await t.finishAllScheduledFunctions(vi.runAllTimers);

  for (const tokenmaxxerId of tokenmaxxerIds.slice(-2)) {
    const finalScores = await t.run(async (ctx) =>
      ctx.db
        .query("publicUsages")
        .withIndex("by_tokenmaxxer_id", (q) =>
          q.eq("tokenmaxxerId", tokenmaxxerId),
        )
        .take(9),
    );
    expect(finalScores).toHaveLength(9);
  }
});

test("a delayed daily recomputation uses its execution Ranking Day", async () => {
  const t = testBackend();
  const beforeMidnight = new Date("2026-08-08T23:59:59.900Z");
  const afterMidnight = new Date("2026-08-09T00:00:00.100Z");
  const tokenmaxxerId = await t.run(async (ctx) => {
    const id = await ctx.db.insert("tokenmaxxers", {
      authSubject: "delayed-rollover",
      createdAt: beforeMidnight.getTime(),
      displayName: "Delayed Tokenmaxxer",
      lastSyncedAt: beforeMidnight.getTime(),
      publicId: "TG-DELAY2",
    });
    await ctx.db.insert("userDailyUsage", {
      apiEquivalentCost: null,
      observedTokens: 100,
      provider: "codex",
      rankingDay: "2026-08-08",
      tokenmaxxerId: id,
      updatedAt: beforeMidnight.getTime(),
    });
    await ctx.db.insert("userDailyUsage", {
      apiEquivalentCost: null,
      observedTokens: 200,
      provider: "codex",
      rankingDay: "2026-08-09",
      tokenmaxxerId: id,
      updatedAt: afterMidnight.getTime(),
    });
    return id;
  });

  vi.setSystemTime(beforeMidnight);
  await t.mutation(internal.internal.recompute.scheduleAll, {});
  vi.setSystemTime(afterMidnight);
  await t.finishAllScheduledFunctions(vi.runAllTimers);

  const currentScore = await t.run(async (ctx) =>
    ctx.db
      .query("publicUsages")
      .withIndex(
        "by_tokenmaxxer_id_and_scope_and_window_days",
        (q) =>
          q
            .eq("tokenmaxxerId", tokenmaxxerId)
            .eq("scope", "combined")
            .eq("windowDays", 1),
      )
      .unique(),
  );
  expect(currentScore).toMatchObject({
    computedAt: afterMidnight.getTime(),
    tokenScore: 200,
  });
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
    profileBackfillAnchor: null,
    activeMacGeneration: 1,
    installationCredential: aliceCredential,
    snapshots: [
      usageSnapshot({ observedTokens: 101 }),
      usageSnapshot({ observedTokens: 102, provider: "claude" }),
    ],
  });
  await bob.authenticated.mutation(api.sync.dailyUsage, {
    profileBackfillAnchor: null,
    activeMacGeneration: 1,
    installationCredential: bobCredential,
    snapshots: [
      usageSnapshot({ observedTokens: 201 }),
      usageSnapshot({ observedTokens: 202, provider: "claude" }),
    ],
  });

  const stored = await t.run(async (ctx) => ({
    publicUsages: await ctx.db.query("publicUsages").collect(),
    tokenmaxxers: await ctx.db.query("tokenmaxxers").collect(),
    usageBuckets: await ctx.db.query("usageBuckets").collect(),
    userDailyUsage: await ctx.db.query("userDailyUsage").collect(),
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
  expect(stored.publicUsages).toHaveLength(18);
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
      profileBackfillAnchor: null,
      activeMacGeneration: 1,
      installationCredential: credential,
      snapshots: [usageSnapshot()],
    }),
  ).rejects.toThrow("authority-rejected");
});

test("an unproved decrease rolls back the whole batch and an explicit correction commits", async () => {
  const t = testBackend();
  const credential = installationCredential("A");
  const { authenticated } = await createProfile(t, credential, "Fabien");
  await authenticated.mutation(api.sync.dailyUsage, {
    profileBackfillAnchor: null,
    activeMacGeneration: 1,
    installationCredential: credential,
    snapshots: [usageSnapshot()],
  });
  const beforeRollback = await t.run(async (ctx) => ({
    publicUsages: await ctx.db.query("publicUsages").collect(),
    usageBuckets: await ctx.db.query("usageBuckets").collect(),
    userDailyUsage: await ctx.db.query("userDailyUsage").collect(),
  }));

  await expect(
    authenticated.mutation(api.sync.dailyUsage, {
      profileBackfillAnchor: null,
      activeMacGeneration: 1,
      installationCredential: credential,
      snapshots: [
        usageSnapshot({ observedTokens: 90, revision: 2 }),
        usageSnapshot({ provider: "claude" }),
      ],
    }),
  ).rejects.toThrow("correction provenance");
  const afterRollback = await t.run(async (ctx) => ({
    publicUsages: await ctx.db.query("publicUsages").collect(),
    usageBuckets: await ctx.db.query("usageBuckets").collect(),
    userDailyUsage: await ctx.db.query("userDailyUsage").collect(),
  }));
  expect(afterRollback).toEqual(beforeRollback);

  await expect(
    authenticated.mutation(api.sync.dailyUsage, {
      profileBackfillAnchor: null,
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
        profileBackfillAnchor: null,
        activeMacGeneration: 1,
        installationCredential: credential,
        snapshots: [snapshot],
      }),
    ).rejects.toThrow("final evidence");
  }
  expect(
    await t.run(async (ctx) => ctx.db.query("usageBuckets").collect()),
  ).toEqual([]);

  await expect(
    authenticated.mutation(api.sync.dailyUsage, {
      profileBackfillAnchor: null,
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

  const buckets = await t.run(async (ctx) =>
    ctx.db.query("usageBuckets").collect(),
  );
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
    profileBackfillAnchor: null,
    activeMacGeneration: 1,
    installationCredential: credential,
    snapshots: [
      usageSnapshot(),
      usageSnapshot({ observedTokens: 200, provider: "claude" }),
    ],
  });
  await authenticated.mutation(api.sync.dailyUsage, {
    profileBackfillAnchor: null,
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
      profileBackfillAnchor: null,
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
        profileBackfillAnchor: null,
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
      ({
        correctionReason,
        correctionRevision,
        observedTokens,
        provider,
        revision,
      }) => ({
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
    profileBackfillAnchor: null,
    activeMacGeneration: 1,
    installationCredential: credential,
    snapshots: [usageSnapshot()],
  });
  await authenticated.mutation(api.sync.dailyUsage, {
    profileBackfillAnchor: null,
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
      profileBackfillAnchor: null,
      activeMacGeneration: 1,
      installationCredential: credential,
      snapshots: [usageSnapshot({ observedTokens: 110, revision: 3 })],
    }),
  ).resolves.toMatchObject([{ outcome: "committed", revision: 3 }]);
  await expect(
    authenticated.mutation(api.sync.dailyUsage, {
      profileBackfillAnchor: null,
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
    profileBackfillAnchor: null,
    activeMacGeneration: 1,
    installationCredential: credential,
    snapshots: [
      usageSnapshot(),
      usageSnapshot({ observedTokens: 200, provider: "claude" }),
    ],
  });

  await expect(
    authenticated.mutation(api.sync.dailyUsage, {
      profileBackfillAnchor: null,
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
    profileBackfillAnchor: null,
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
        profileBackfillAnchor: null,
        activeMacGeneration: 1,
        installationCredential: credential,
        snapshots: [snapshot],
      }),
    ).rejects.toThrow();
  }

  await expect(
    authenticated.mutation(api.sync.dailyUsage, {
      profileBackfillAnchor: null,
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
  ]) {
    await expect(
      authenticated.mutation(api.sync.dailyUsage, {
        profileBackfillAnchor: null,
        activeMacGeneration: 1,
        installationCredential: credential,
        snapshots: [snapshot],
      }),
    ).rejects.toThrow();
  }

  await expect(
    authenticated.mutation(api.sync.dailyUsage, {
      profileBackfillAnchor: null,
      activeMacGeneration: 1,
      installationCredential: credential,
      snapshots: [
        usageSnapshot({
          correctionReason: "parser-correction",
          correctionRevision: 2,
          observedTokens: 80,
          revision: 3,
        }),
      ],
    }),
  ).resolves.toEqual([
    {
      outcome: "stale",
      provider: "codex",
      rankingDay: TODAY,
      revision: 3,
    },
  ]);

  const buckets = await t.run(async (ctx) =>
    ctx.db.query("usageBuckets").collect(),
  );
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
      profileBackfillAnchor: null,
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
          ? "openai-api-2026-08-09-v3"
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
        [
          usageSnapshot({
            observedTokens: Number.MAX_SAFE_INTEGER + 1,
            provider,
          }),
        ],
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
        profileBackfillAnchor: null,
        ...baseArgs,
        snapshots,
      } as never),
    ).rejects.toThrow();
  }
  await expect(
    authenticated.mutation(api.sync.dailyUsage, {
      profileBackfillAnchor: null,
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
      publicUsages: await ctx.db.query("publicUsages").collect(),
      usageBuckets: await ctx.db.query("usageBuckets").collect(),
      userDailyUsage: await ctx.db.query("userDailyUsage").collect(),
    })),
  ).toEqual({
    publicUsages: [],
    usageBuckets: [],
    userDailyUsage: [],
  });
});

test("the app database stores only the installation digest and approved usage fields", async () => {
  const t = testBackend();
  const credential = installationCredential("A");
  const { authenticated } = await createProfile(t, credential, "Fabien");
  await authenticated.mutation(api.sync.dailyUsage, {
    profileBackfillAnchor: null,
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
    publicUsages: await ctx.db.query("publicUsages").collect(),
    tokenmaxxers: await ctx.db.query("tokenmaxxers").collect(),
    usageCorrectionAudits: await ctx.db
      .query("usageCorrectionAudits")
      .collect(),
    usageBuckets: await ctx.db.query("usageBuckets").collect(),
    userDailyUsage: await ctx.db.query("userDailyUsage").collect(),
  }));
  const serialized = JSON.stringify(stored);
  expect(serialized).not.toContain(credential);
  expect(serialized).not.toContain("privatePath");
  expect(serialized).not.toContain("providerMessageId");
  expect(stored.devices[0]?.installationCredentialDigest).toMatch(
    /^sha256:[0-9a-f]{64}$/,
  );
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
