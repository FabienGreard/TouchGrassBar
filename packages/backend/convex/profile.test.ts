/// <reference types="vite/client" />

import betterAuthTest from "@convex-dev/better-auth/test";
import doomerboardIndexTest from "@convex-dev/aggregate/test";
import rateLimiterTest from "@convex-dev/rate-limiter/test";
import { convexTest } from "convex-test";
import { afterEach, expect, test, vi } from "vitest";

import { api, components, internal } from "./_generated/api";
import { createAuthWithRequestIp } from "./auth";
import { installationCredentialDigest } from "./model/profile";
import { touchGrassAuthPolicy } from "./model/rateLimits";
import schema from "./schema";

const modules = import.meta.glob("./**/*.ts");
const DEFAULT_PLATFORM_IP = "203.0.113.10";
const INSTALLATION_CREDENTIAL = "A".repeat(52);

function testBackend() {
  const t = convexTest(schema, modules);
  betterAuthTest.register(t);
  doomerboardIndexTest.register(t, "doomerboard");
  rateLimiterTest.register(t);
  return t;
}

async function authFetch(
  t: ReturnType<typeof testBackend>,
  path: string,
  init: RequestInit,
  platformIp = DEFAULT_PLATFORM_IP,
) {
  const result = await t.action(async (ctx) => {
    const auth = createAuthWithRequestIp(ctx, async () => platformIp);
    const response = await auth.handler(new Request(`https://example.convex.site${path}`, init));
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

function jwtPayload(token: string) {
  const encoded = token.split(".")[1];
  if (!encoded) throw new Error("JWT payload is missing");
  const normalized = encoded.replaceAll("-", "+").replaceAll("_", "/");
  const padded = normalized.padEnd(Math.ceil(normalized.length / 4) * 4, "=");
  return JSON.parse(atob(padded)) as { sessionId?: unknown; sub?: unknown };
}

async function json(response: Response) {
  return (await response.json()) as Record<string, unknown>;
}

async function createRecoverableProfile(
  t: ReturnType<typeof testBackend>,
  displayName = "Fabien",
) {
  const recoveryKey = "R".repeat(48);
  const preparation = await authFetch(t, "/api/auth/touchgrass/prepare", {
    method: "POST",
  });
  const prepared = await json(preparation);
  const touchGrassId = String(prepared.touchGrassId);
  const signup = await authFetch(t, "/api/auth/sign-up/email", {
    body: JSON.stringify({
      email: `${touchGrassId.toLowerCase()}@profile.touchgrass.invalid`,
      name: displayName,
      password: recoveryKey,
      username: touchGrassId,
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
    body: JSON.stringify({ password: recoveryKey, username: touchGrassId }),
    headers: { "content-type": "application/json" },
    method: "POST",
  });
  expect(signIn.status).toBe(200);
  const session = String((await json(signIn)).token);
  const tokenResponse = await authFetch(t, "/api/auth/convex/token", {
    headers: bearer(session),
  });
  expect(tokenResponse.status).toBe(200);
  const payload = jwtPayload(String((await json(tokenResponse)).token));
  const authenticated = t.withIdentity({
    sessionId: payload.sessionId as string,
    subject: user.id,
    tokenIdentifier: `touchgrass|${user.id}`,
  });
  await authenticated.mutation(api.tokenmaxxers.ensureProfile, {
    displayName,
    expectedTouchGrassId: touchGrassId,
    installationCredential: INSTALLATION_CREDENTIAL,
  });
  return {
    authenticated,
    displayName,
    recoveryKey,
    session,
    sessionId: String(payload.sessionId),
    touchGrassId,
    user,
  };
}

function recoveryAttemptId(character: string) {
  return character.repeat(32);
}

function replacementRecoveryKey(character: string) {
  return character.repeat(48);
}

async function recoveryDigest(value: string) {
  const digest = new Uint8Array(
    await crypto.subtle.digest("SHA-256", new TextEncoder().encode(value)),
  );
  const binary = Array.from(digest, (byte) => String.fromCharCode(byte)).join("");
  return btoa(binary)
    .replaceAll("+", "-")
    .replaceAll("/", "_")
    .replaceAll("=", "");
}

async function prepareRecovery(
  t: ReturnType<typeof testBackend>,
  profile: Awaited<ReturnType<typeof createRecoverableProfile>>,
  attemptId: string,
  stagedRecoveryKey: string,
  platformIp = DEFAULT_PLATFORM_IP,
) {
  const response = await authFetch(
    t,
    "/api/auth/touchgrass/recovery/prepare",
    {
      body: JSON.stringify({
        attemptId,
        recoveryKey: profile.recoveryKey,
        replacementRecoveryKey: stagedRecoveryKey,
        touchGrassId: profile.touchGrassId,
      }),
      headers: { "content-type": "application/json" },
      method: "POST",
    },
    platformIp,
  );
  expect(response.status).toBe(200);
  return String((await json(response)).recoveryProof);
}

afterEach(() => {
  vi.unstubAllEnvs();
  vi.useRealTimers();
});

test("Recovery Key signup is short-lived, hashed, and session-validating", async () => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date("2026-08-05T00:00:00.000Z"));
  vi.stubEnv("BETTER_AUTH_SECRET", `${crypto.randomUUID()}${crypto.randomUUID()}`);
  vi.stubEnv("CONVEX_SITE_URL", "https://example.convex.site");

  const t = testBackend();

  const expiredPreparation = await authFetch(t, "/api/auth/touchgrass/prepare", { method: "POST" });
  expect(expiredPreparation.status).toBe(200);
  const expired = await json(expiredPreparation);
  vi.advanceTimersByTime(121_000);

  const recoveryKey = `${crypto.randomUUID()}${crypto.randomUUID()}`;
  const expiredSignup = await authFetch(t, "/api/auth/sign-up/email", {
    body: JSON.stringify({
      email: `${String(expired.touchGrassId).toLowerCase()}@profile.touchgrass.invalid`,
      name: "Fabien",
      password: recoveryKey,
      username: expired.touchGrassId,
    }),
    headers: {
      "content-type": "application/json",
      "x-touchgrass-signup-proof": String(expired.signupProof),
    },
    method: "POST",
  });
  expect(expiredSignup.status).toBe(403);

  const preparation = await authFetch(t, "/api/auth/touchgrass/prepare", {
    method: "POST",
  });
  const prepared = await json(preparation);
  expect(prepared.touchGrassId).toMatch(/^TG-[A-HJ-NP-Z2-9]{6}$/);
  expect(typeof prepared.signupProof).toBe("string");

  const signupBody = {
    email: `${String(prepared.touchGrassId).toLowerCase()}@profile.touchgrass.invalid`,
    name: "Fabien",
    password: recoveryKey,
    username: prepared.touchGrassId,
  };
  const missingProof = await authFetch(t, "/api/auth/sign-up/email", {
    body: JSON.stringify(signupBody),
    headers: { "content-type": "application/json" },
    method: "POST",
  });
  expect(missingProof.status).toBe(403);

  const signupRequest = () =>
    authFetch(t, "/api/auth/sign-up/email", {
      body: JSON.stringify(signupBody),
      headers: {
        "content-type": "application/json",
        "x-touchgrass-signup-proof": String(prepared.signupProof),
      },
      method: "POST",
    });
  const concurrentSignups = await Promise.all([signupRequest(), signupRequest()]);
  expect(concurrentSignups.map(({ status }) => status).sort()).toEqual([200, 403]);
  const signup = concurrentSignups.find(({ status }) => status === 200);
  expect(signup).toBeDefined();
  if (!signup) throw new Error("Concurrent signup did not succeed");
  const replay = await signupRequest();
  expect(replay.status).toBe(403);
  const signupResult = await json(signup);
  const user = signupResult.user as { id: string };

  const emailSignIn = await authFetch(t, "/api/auth/sign-in/email", {
    body: JSON.stringify({
      email: signupBody.email,
      password: recoveryKey,
    }),
    headers: { "content-type": "application/json" },
    method: "POST",
  });
  expect(emailSignIn.status).toBe(404);

  const credential = await t.run((ctx) =>
    ctx.runQuery(components.betterAuth.adapter.findOne, {
      model: "account",
      where: [
        { field: "userId", value: user.id },
        { field: "providerId", value: "credential" },
      ],
    }),
  );
  expect(credential?.password).not.toBe(recoveryKey);
  expect(credential?.password).toMatch(/^[0-9a-f]{32}:[0-9a-f]{128}$/);

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

  const updateTouchGrassId = await authFetch(t, "/api/auth/update-user", {
    body: JSON.stringify({ username: "TG-ZZZZ22" }),
    headers: {
      ...bearer(sessionToken),
      "content-type": "application/json",
    },
    method: "POST",
  });
  expect(updateTouchGrassId.status).toBe(404);

  const tokenResponse = await authFetch(t, "/api/auth/convex/token", {
    headers: bearer(sessionToken),
  });
  expect(tokenResponse.status).toBe(200);
  const { token: convexJwt } = (await json(tokenResponse)) as { token: string };
  const payload = jwtPayload(convexJwt);
  expect(payload.sub).toBe(user.id);
  expect(typeof payload.sessionId).toBe("string");

  const authenticated = t.withIdentity({
    sessionId: payload.sessionId as string,
    subject: user.id,
    tokenIdentifier: `touchgrass|${user.id}`,
  });
  await expect(
    authenticated.mutation(api.tokenmaxxers.ensureProfile, {
      displayName: "Fabien",
      expectedTouchGrassId: String(prepared.touchGrassId),
      installationCredential: "0".repeat(52),
    }),
  ).rejects.toMatchObject({
    data: { code: "authority-rejected" },
    name: "ConvexError",
  });
  expect(
    await t.run(async (ctx) => ({
      devices: await ctx.db.query("devices").collect(),
      tokenmaxxers: await ctx.db.query("tokenmaxxers").collect(),
    })),
  ).toEqual({ devices: [], tokenmaxxers: [] });
  const activeMacActivatedAt = Date.now();
  await expect(
    authenticated.mutation(api.tokenmaxxers.ensureProfile, {
      displayName: "Fabien",
      expectedTouchGrassId: String(prepared.touchGrassId),
      installationCredential: INSTALLATION_CREDENTIAL,
    }),
  ).resolves.toEqual({
    activeMacActivatedAt,
    activeMacGeneration: 1,
    displayName: "Fabien",
    touchGrassId: prepared.touchGrassId,
  });
  await expect(
    authenticated.mutation(api.tokenmaxxers.updateDisplayName, {
      displayName: "New name",
    }),
  ).resolves.toEqual({
    displayName: "New name",
    touchGrassId: prepared.touchGrassId,
  });

  const signOut = await authFetch(t, "/api/auth/sign-out", {
    headers: bearer(sessionToken),
    method: "POST",
  });
  expect(signOut.status).toBe(200);
  await expect(
    authenticated.mutation(api.tokenmaxxers.ensureProfile, {
      displayName: "Fabien",
      expectedTouchGrassId: String(prepared.touchGrassId),
      installationCredential: INSTALLATION_CREDENTIAL,
    }),
  ).rejects.toThrow("authority-rejected");
});

test("failed Recovery Keys use independent opaque limits", async () => {
  vi.stubEnv("BETTER_AUTH_SECRET", `${crypto.randomUUID()}${crypto.randomUUID()}`);
  vi.stubEnv("CONVEX_SITE_URL", "https://example.convex.site");

  const t = testBackend();

  async function createCredential() {
    const recoveryKey = `${crypto.randomUUID()}${crypto.randomUUID()}`;
    const preparation = await authFetch(t, "/api/auth/touchgrass/prepare", {
      method: "POST",
    });
    const prepared = await json(preparation);
    const signup = await authFetch(t, "/api/auth/sign-up/email", {
      body: JSON.stringify({
        email: `${String(prepared.touchGrassId).toLowerCase()}@profile.touchgrass.invalid`,
        name: "Fabien",
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
    return { recoveryKey, touchGrassId: String(prepared.touchGrassId) };
  }

  async function signIn(
    touchGrassId: string,
    recoveryKey: string,
    platformIp: string,
    forwardedIp = "198.18.0.1",
  ) {
    return authFetch(
      t,
      "/api/auth/sign-in/username",
      {
        body: JSON.stringify({ password: recoveryKey, username: touchGrassId }),
        headers: {
          "content-type": "application/json",
          "x-forwarded-for": forwardedIp,
        },
        method: "POST",
      },
      platformIp,
    );
  }

  const byIpCredential = await createCredential();
  const byIdCredential = await createCredential();
  const wrongRecoveryKey = `${crypto.randomUUID()}${crypto.randomUUID()}`;
  const attempts = touchGrassAuthPolicy.failedRecoveryKey.attempts;

  const successfulBeforeFailures = await signIn(
    byIpCredential.touchGrassId,
    byIpCredential.recoveryKey,
    "192.0.2.1",
  );
  expect(successfulBeforeFailures.status).toBe(200);

  const sameIpFailures: Response[] = [];
  for (let attempt = 0; attempt < attempts - 1; attempt += 1) {
    sameIpFailures.push(
      await signIn(
        `TG-AAAA${22 + attempt}`,
        wrongRecoveryKey,
        "192.0.2.1",
        `198.18.0.${attempt + 1}`,
      ),
    );
  }
  const successfulAtBoundary = await signIn(
    byIpCredential.touchGrassId,
    byIpCredential.recoveryKey,
    "192.0.2.1",
  );
  expect(successfulAtBoundary.status).toBe(200);
  sameIpFailures.push(
    await signIn(`TG-AAAA${21 + attempts}`, wrongRecoveryKey, "192.0.2.1", "198.18.0.100"),
  );
  expect(sameIpFailures.every(({ status }) => status === 401)).toBe(true);
  const blockedByIp = await signIn(
    byIpCredential.touchGrassId,
    byIpCredential.recoveryKey,
    "192.0.2.1",
    "198.18.0.200",
  );
  expect(blockedByIp.status).toBe(429);

  const distinctIpFailures: Response[] = [];
  for (let attempt = 0; attempt < attempts; attempt += 1) {
    distinctIpFailures.push(
      await signIn(byIdCredential.touchGrassId, wrongRecoveryKey, `198.51.100.${attempt + 1}`),
    );
  }
  expect(distinctIpFailures.every(({ status }) => status === 401)).toBe(true);
  const blockedById = await signIn(
    byIdCredential.touchGrassId,
    byIdCredential.recoveryKey,
    "198.51.100.200",
  );
  expect(blockedById.status).toBe(429);
});

test("Recovery Key attempt admission is atomic and resets at the typed boundary", async () => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date("2026-08-05T00:00:00.000Z"));

  const t = testBackend();
  const keys = {
    ipKey: crypto.randomUUID(),
    touchGrassIdKey: crypto.randomUUID(),
  };
  const attempts = touchGrassAuthPolicy.failedRecoveryKey.attempts;
  const admitted = await Promise.all(
    Array.from({ length: attempts + 1 }, () =>
      t.mutation(internal.auth.credentialAttempts.reserveCredentialAttempt, keys),
    ),
  );
  const reservationIds = admitted.filter((reservationId) => reservationId !== null);
  expect(reservationIds).toHaveLength(attempts);

  await t.mutation(internal.auth.credentialAttempts.finalizeCredentialAttempt, {
    outcome: "success",
    reservationId: reservationIds[0]!,
  });
  const replacementId = await t.mutation(
    internal.auth.credentialAttempts.reserveCredentialAttempt,
    keys,
  );
  expect(replacementId).not.toBeNull();
  if (!replacementId) throw new Error("Replacement reservation is missing");

  for (const reservationId of [...reservationIds.slice(1, attempts), replacementId]) {
    await t.mutation(internal.auth.credentialAttempts.finalizeCredentialAttempt, {
      outcome: "failure",
      reservationId,
    });
  }
  await expect(
    t.mutation(internal.auth.credentialAttempts.reserveCredentialAttempt, keys),
  ).resolves.toBeNull();

  vi.advanceTimersByTime(touchGrassAuthPolicy.failedRecoveryKey.windowMs + 1);
  await expect(
    t.mutation(internal.auth.credentialAttempts.reserveCredentialAttempt, keys),
  ).resolves.not.toBeNull();
});

test("Profile preparation uses the typed IP boundary and reset window", async () => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date("2026-08-05T00:00:00.000Z"));
  vi.stubEnv("BETTER_AUTH_SECRET", `${crypto.randomUUID()}${crypto.randomUUID()}`);
  vi.stubEnv("CONVEX_SITE_URL", "https://example.convex.site");

  const t = testBackend();
  const policy = touchGrassAuthPolicy.profilePreparation;
  const attempts = await Promise.all(
    Array.from({ length: policy.attempts }, () =>
      authFetch(t, "/api/auth/touchgrass/prepare", { method: "POST" }),
    ),
  );
  expect(attempts.every(({ status }) => status === 200)).toBe(true);
  await expect(
    authFetch(t, "/api/auth/touchgrass/prepare", { method: "POST" }),
  ).resolves.toMatchObject({ status: 429 });

  vi.advanceTimersByTime(policy.windowMs + 1);
  await expect(
    authFetch(t, "/api/auth/touchgrass/prepare", { method: "POST" }),
  ).resolves.toMatchObject({ status: 200 });
});

test("Profile recovery is idempotent and rotates one-writer authority only at commit", async () => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date("2026-08-09T12:00:00.000Z"));
  vi.stubEnv(
    "BETTER_AUTH_SECRET",
    `${crypto.randomUUID()}${crypto.randomUUID()}`,
  );
  vi.stubEnv("CONVEX_SITE_URL", "https://example.convex.site");

  const t = testBackend();
  const profile = await createRecoverableProfile(t);
  const attemptId = recoveryAttemptId("A");
  const newRecoveryKey = replacementRecoveryKey("B");
  const firstProof = await prepareRecovery(t, profile, attemptId, newRecoveryKey);
  const repeatedProof = await prepareRecovery(
    t,
    profile,
    attemptId,
    newRecoveryKey,
  );
  expect(repeatedProof).toBe(firstProof);

  await expect(
    profile.authenticated.mutation(api.tokenmaxxers.ensureProfile, {
      displayName: profile.displayName,
      expectedTouchGrassId: profile.touchGrassId,
      installationCredential: INSTALLATION_CREDENTIAL,
    }),
  ).resolves.toMatchObject({ activeMacGeneration: 1 });
  await expect(
    authFetch(t, "/api/auth/convex/token", {
      headers: bearer(profile.session),
    }),
  ).resolves.toMatchObject({ status: 200 });
  await profile.authenticated.mutation(api.sync.dailyUsage, {
    activeMacGeneration: 1,
    installationCredential: INSTALLATION_CREDENTIAL,
    profileBackfillAnchor: null,
    snapshots: [
      {
        apiEquivalentCost: null,
        correctionReason: null,
        correctionRevision: null,
        coverage: "complete",
        evidenceBasis: "locally-derived",
        observedAt: Date.now(),
        observedTokens: 100,
        provider: "codex",
        rankingDay: "2026-08-09",
        revision: 1,
      },
    ],
  });

  const newInstallationCredential = "C".repeat(52);
  const commitBody = {
    currentRecoveryKey: profile.recoveryKey,
    installationCredential: newInstallationCredential,
    newRecoveryKey,
    recoveryProof: firstProof,
  };
  const commit = await authFetch(
    t,
    "/api/auth/touchgrass/recovery/commit",
    {
      body: JSON.stringify(commitBody),
      headers: { "content-type": "application/json" },
      method: "POST",
    },
  );
  expect(commit.status).toBe(200);
  const committed = await json(commit);
  expect(committed).toMatchObject({
    activeMacActivatedAt: Date.now(),
    activeMacGeneration: 2,
    displayName: profile.displayName,
    touchGrassId: profile.touchGrassId,
  });
  expect(committed).not.toHaveProperty("token");

  await expect(
    authFetch(t, "/api/auth/convex/token", {
      headers: bearer(profile.session),
    }),
  ).resolves.toMatchObject({ status: 401 });
  await expect(
    authFetch(t, "/api/auth/sign-in/username", {
      body: JSON.stringify({
        password: profile.recoveryKey,
        username: profile.touchGrassId,
      }),
      headers: { "content-type": "application/json" },
      method: "POST",
    }),
  ).resolves.toMatchObject({ status: 401 });
  await expect(
    profile.authenticated.mutation(api.tokenmaxxers.ensureProfile, {
      displayName: profile.displayName,
      expectedTouchGrassId: profile.touchGrassId,
      installationCredential: INSTALLATION_CREDENTIAL,
    }),
  ).rejects.toThrow("authority-rejected");

  const mismatchedReplay = await authFetch(
    t,
    "/api/auth/touchgrass/recovery/commit",
    {
      body: JSON.stringify({
        ...commitBody,
        currentRecoveryKey: newRecoveryKey,
        installationCredential: "D".repeat(52),
      }),
      headers: { "content-type": "application/json" },
      method: "POST",
    },
  );
  expect(mismatchedReplay.status).toBe(401);

  const newSignIn = await authFetch(t, "/api/auth/sign-in/username", {
    body: JSON.stringify({
      password: newRecoveryKey,
      username: profile.touchGrassId,
    }),
    headers: { "content-type": "application/json" },
    method: "POST",
  });
  expect(newSignIn.status).toBe(200);
  const newSession = String((await json(newSignIn)).token);

  vi.advanceTimersByTime(5 * 60 * 1_000 + 1);
  const refreshedProof = await prepareRecovery(
    t,
    profile,
    attemptId,
    newRecoveryKey,
  );
  const retry = await authFetch(
    t,
    "/api/auth/touchgrass/recovery/commit",
    {
      body: JSON.stringify({
        ...commitBody,
        currentRecoveryKey: newRecoveryKey,
        recoveryProof: refreshedProof,
      }),
      headers: { "content-type": "application/json" },
      method: "POST",
    },
  );
  expect(retry.status).toBe(200);
  expect(await json(retry)).toMatchObject({
    activeMacActivatedAt: committed.activeMacActivatedAt,
    activeMacGeneration: 2,
    displayName: profile.displayName,
    touchGrassId: profile.touchGrassId,
  });
  await expect(
    authFetch(t, "/api/auth/convex/token", {
      headers: bearer(newSession),
    }),
  ).resolves.toMatchObject({ status: 200 });

  const stored = await t.run(async (ctx) => ({
    devices: await ctx.db.query("devices").collect(),
    recoveryAttempts: await ctx.db.query("profileRecoveryAttempts").collect(),
    transferBoundaries: await ctx.db.query("usageTransferBoundaries").collect(),
    usageBuckets: await ctx.db.query("usageBuckets").collect(),
  }));
  expect(stored.devices).toHaveLength(2);
  expect(stored.devices).toContainEqual(
    expect.objectContaining({
      generation: 1,
      revokedAt: committed.activeMacActivatedAt,
    }),
  );
  expect(stored.devices).toContainEqual(
    expect.objectContaining({
      generation: 2,
      installationCredentialDigest: expect.stringMatching(/^sha256:/),
    }),
  );
  expect(stored.devices.find(({ generation }) => generation === 2)).not.toHaveProperty(
    "revokedAt",
  );
  const oldDevice = stored.devices.find(({ generation }) => generation === 1);
  expect(stored.recoveryAttempts).toContainEqual(
    expect.objectContaining({
      authFinalizedAt: committed.activeMacActivatedAt,
      status: "committed",
    }),
  );
  expect(stored.transferBoundaries).toEqual(
    expect.arrayContaining([
      expect.objectContaining({
        previousDeviceId: oldDevice?._id,
        provider: "codex",
        rankingDay: "2026-08-09",
      }),
      expect.objectContaining({
        previousDeviceId: oldDevice?._id,
        provider: "claude",
        rankingDay: "2026-08-09",
      }),
    ]),
  );
  expect(stored.usageBuckets).toContainEqual(
    expect.objectContaining({
      coverage: "partial",
      deviceId: oldDevice?._id,
      observedTokens: 100,
    }),
  );
  expect(JSON.stringify(stored)).not.toContain(profile.recoveryKey);
  expect(JSON.stringify(stored)).not.toContain(newRecoveryKey);
  expect(JSON.stringify(stored)).not.toContain(newInstallationCredential);
  expect(JSON.stringify(stored)).not.toContain(attemptId);
});

test("concurrent identical recovery commits finalize authentication once", async () => {
  vi.stubEnv(
    "BETTER_AUTH_SECRET",
    `${crypto.randomUUID()}${crypto.randomUUID()}`,
  );
  vi.stubEnv("CONVEX_SITE_URL", "https://example.convex.site");

  const t = testBackend();
  const profile = await createRecoverableProfile(t);
  const attemptId = recoveryAttemptId("J");
  const newRecoveryKey = replacementRecoveryKey("K");
  const installationCredential = "M".repeat(52);
  const recoveryProof = await prepareRecovery(
    t,
    profile,
    attemptId,
    newRecoveryKey,
  );
  const attemptDigest = await recoveryDigest(attemptId);
  await expect(
    t.query(internal.auth.profileRecovery.profileAuthGeneration, {
      touchGrassId: profile.touchGrassId,
    }),
  ).resolves.toBe(1);
  await t.mutation(internal.auth.profileRecovery.claimRecoveryAttempt, {
    attemptDigest,
    authSubject: profile.user.id,
    installationCredentialDigest: await installationCredentialDigest(
      installationCredential,
    ),
    replacementRecoveryKeyDigest: await recoveryDigest(newRecoveryKey),
  });
  await expect(
    t.mutation(internal.auth.profileRecovery.commitRecoveryAttempt, {
      attemptDigest,
      authSubject: profile.user.id,
      installationCredential,
    }),
  ).resolves.toMatchObject({ authFinalized: false });
  await expect(
    t.query(internal.auth.profileRecovery.profileAuthGeneration, {
      touchGrassId: profile.touchGrassId,
    }),
  ).resolves.toBe(2);

  const claims = await Promise.all(
    ["first-concurrent-claim", "second-concurrent-claim"].map((claim) =>
      t.mutation(
        internal.auth.profileRecovery.claimRecoveryAuthFinalization,
        {
          attemptDigest,
          authSubject: profile.user.id,
          claim,
        },
      ),
    ),
  );
  expect(claims.filter(Boolean)).toHaveLength(1);
  const winningClaim = claims[0]
    ? "first-concurrent-claim"
    : "second-concurrent-claim";
  await t.run(async (ctx) => {
    const attempt = await ctx.db
      .query("profileRecoveryAttempts")
      .withIndex("by_attempt_digest", (query) =>
        query.eq("attemptDigest", attemptDigest),
      )
      .unique();
    if (!attempt) throw new Error("Recovery attempt is missing");
    await ctx.db.patch(attempt._id, {
      authFinalizationLeaseExpiresAt: Date.now() - 1,
    });
  });
  const replacementClaim = "replacement-after-expired-lease";
  await expect(
    t.mutation(internal.auth.profileRecovery.claimRecoveryAuthFinalization, {
      attemptDigest,
      authSubject: profile.user.id,
      claim: replacementClaim,
    }),
  ).resolves.toBe(true);
  await expect(
    t.mutation(internal.auth.profileRecovery.claimRecoveryAuthFinalization, {
      attemptDigest,
      authSubject: profile.user.id,
      claim: winningClaim,
    }),
  ).resolves.toBe(false);

  const overlappingReplay = authFetch(
    t,
    "/api/auth/touchgrass/recovery/commit",
    {
      body: JSON.stringify({
        currentRecoveryKey: profile.recoveryKey,
        installationCredential,
        newRecoveryKey,
        recoveryProof,
      }),
      headers: { "content-type": "application/json" },
      method: "POST",
    },
  );
  await new Promise((resolve) => setTimeout(resolve, 50));
  await expect(
    t.mutation(internal.auth.profileRecovery.finalizeRecoveryAuth, {
      attemptDigest,
      authSubject: profile.user.id,
      claim: replacementClaim,
    }),
  ).resolves.toBe(true);
  await expect(overlappingReplay).resolves.toMatchObject({ status: 200 });
  await expect(
    t.mutation(internal.auth.profileRecovery.claimRecoveryAuthFinalization, {
      attemptDigest,
      authSubject: profile.user.id,
      claim: "late-replay-claim",
    }),
  ).resolves.toBe(false);
});

test("a stale sign-in cannot replace the recovered Profile session fence", async () => {
  vi.stubEnv(
    "BETTER_AUTH_SECRET",
    `${crypto.randomUUID()}${crypto.randomUUID()}`,
  );
  vi.stubEnv("CONVEX_SITE_URL", "https://example.convex.site");

  const t = testBackend();
  const profile = await createRecoverableProfile(t);
  await t.run(async (ctx) => {
    for (let index = 0; index < 129; index += 1) {
      await ctx.runMutation(components.betterAuth.adapter.create, {
        input: {
          data: {
            createdAt: Date.now(),
            expiresAt: Date.now() + 60_000,
            token: `bounded-recovery-session-${index}`,
            updatedAt: Date.now(),
            userId: profile.user.id,
          },
          model: "session",
        },
      });
    }
  });
  const attemptId = recoveryAttemptId("S");
  const newRecoveryKey = replacementRecoveryKey("T");
  const installationCredential = "U".repeat(52);
  const recoveryProof = await prepareRecovery(
    t,
    profile,
    attemptId,
    newRecoveryKey,
  );
  const commit = (currentRecoveryKey: string) =>
    authFetch(t, "/api/auth/touchgrass/recovery/commit", {
      body: JSON.stringify({
        currentRecoveryKey,
        installationCredential,
        newRecoveryKey,
        recoveryProof,
      }),
      headers: { "content-type": "application/json" },
      method: "POST",
    });
  await expect(commit(profile.recoveryKey)).resolves.toMatchObject({
    status: 401,
  });
  await expect(commit(newRecoveryKey)).resolves.toMatchObject({ status: 200 });
  await expect(
    t.run((ctx) =>
      ctx.runQuery(components.betterAuth.adapter.findMany, {
        model: "session",
        paginationOpts: { cursor: null, numItems: 200 },
        where: [{ field: "userId", value: profile.user.id }],
      }),
    ),
  ).resolves.toMatchObject({ isDone: true, page: [] });

  await expect(
    t.mutation(internal.auth.profileRecovery.authorizeProfileSession, {
      activeMacGeneration: 1,
      authSubject: profile.user.id,
      sessionId: profile.sessionId,
      touchGrassId: profile.touchGrassId,
    }),
  ).resolves.toBe(false);
  await expect(
    t.query(internal.auth.profileRecovery.profileSessionAuthorized, {
      authSubject: profile.user.id,
      sessionId: profile.sessionId,
    }),
  ).resolves.toBe(false);

  await expect(
    t.mutation(internal.auth.profileRecovery.authorizeProfileSession, {
      activeMacGeneration: 2,
      authSubject: profile.user.id,
      sessionId: "recovered-session-id",
      touchGrassId: profile.touchGrassId,
    }),
  ).resolves.toBe(true);
  await expect(
    t.query(internal.auth.profileRecovery.profileSessionAuthorized, {
      authSubject: profile.user.id,
      sessionId: "recovered-session-id",
    }),
  ).resolves.toBe(true);
});

test("concurrent Profile recoveries are first-valid-commit-wins", async () => {
  vi.stubEnv(
    "BETTER_AUTH_SECRET",
    `${crypto.randomUUID()}${crypto.randomUUID()}`,
  );
  vi.stubEnv("CONVEX_SITE_URL", "https://example.convex.site");

  const t = testBackend();
  const profile = await createRecoverableProfile(t);
  const attempts = await Promise.all(
    ["D", "E"].map(async (character, index) => ({
      installationCredential: character.repeat(52),
      newRecoveryKey: replacementRecoveryKey(character),
      proof: await prepareRecovery(
        t,
        profile,
        recoveryAttemptId(character),
        replacementRecoveryKey(character),
        `198.51.100.${index + 1}`,
      ),
    })),
  );
  const commits = await Promise.all(
    attempts.map((attempt, index) =>
      authFetch(
        t,
        "/api/auth/touchgrass/recovery/commit",
        {
          body: JSON.stringify({
            currentRecoveryKey: profile.recoveryKey,
            ...attempt,
            proof: undefined,
            recoveryProof: attempt.proof,
          }),
          headers: { "content-type": "application/json" },
          method: "POST",
        },
        `203.0.113.${index + 1}`,
      ),
    ),
  );
  expect(commits.map(({ status }) => status).sort()).toEqual([200, 401]);
  const winner = commits.findIndex(({ status }) => status === 200);
  const loser = winner === 0 ? 1 : 0;
  await expect(
    authFetch(t, "/api/auth/sign-in/username", {
      body: JSON.stringify({
        password: attempts[winner]!.newRecoveryKey,
        username: profile.touchGrassId,
      }),
      headers: { "content-type": "application/json" },
      method: "POST",
    }),
  ).resolves.toMatchObject({ status: 200 });
  await expect(
    authFetch(t, "/api/auth/sign-in/username", {
      body: JSON.stringify({
        password: attempts[loser]!.newRecoveryKey,
        username: profile.touchGrassId,
      }),
      headers: { "content-type": "application/json" },
      method: "POST",
    }),
  ).resolves.toMatchObject({ status: 401 });
});

test("an expired in-flight recovery can refresh its proof and commit", async () => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date("2026-08-09T12:00:00.000Z"));
  vi.stubEnv(
    "BETTER_AUTH_SECRET",
    `${crypto.randomUUID()}${crypto.randomUUID()}`,
  );
  vi.stubEnv("CONVEX_SITE_URL", "https://example.convex.site");

  const t = testBackend();
  const profile = await createRecoverableProfile(t);
  const attemptId = recoveryAttemptId("P");
  const newRecoveryKey = replacementRecoveryKey("M");
  const installationCredential = "N".repeat(52);
  await prepareRecovery(t, profile, attemptId, newRecoveryKey);
  await expect(
    t.mutation(internal.auth.profileRecovery.claimRecoveryAttempt, {
      attemptDigest: await recoveryDigest(attemptId),
      authSubject: profile.user.id,
      installationCredentialDigest: await installationCredentialDigest(
        installationCredential,
      ),
      replacementRecoveryKeyDigest: await recoveryDigest(newRecoveryKey),
    }),
  ).resolves.toBe(true);

  vi.advanceTimersByTime(5 * 60 * 1_000 + 1);
  const refreshedProof = await prepareRecovery(
    t,
    profile,
    attemptId,
    newRecoveryKey,
  );
  const committed = await authFetch(
    t,
    "/api/auth/touchgrass/recovery/commit",
    {
      body: JSON.stringify({
        currentRecoveryKey: profile.recoveryKey,
        installationCredential,
        newRecoveryKey,
        recoveryProof: refreshedProof,
      }),
      headers: { "content-type": "application/json" },
      method: "POST",
    },
  );
  expect(committed.status).toBe(200);
  expect(await json(committed)).toMatchObject({ activeMacGeneration: 2 });
});

test("an unfinalized recovery blocks the new Active Mac and Profile sign-in", async () => {
  vi.stubEnv(
    "BETTER_AUTH_SECRET",
    `${crypto.randomUUID()}${crypto.randomUUID()}`,
  );
  vi.stubEnv("CONVEX_SITE_URL", "https://example.convex.site");

  const t = testBackend();
  const profile = await createRecoverableProfile(t);
  const attemptId = recoveryAttemptId("U");
  const newRecoveryKey = replacementRecoveryKey("V");
  const installationCredential = "W".repeat(52);
  await prepareRecovery(t, profile, attemptId, newRecoveryKey);
  const attemptDigest = await recoveryDigest(attemptId);
  await t.mutation(internal.auth.profileRecovery.claimRecoveryAttempt, {
    attemptDigest,
    authSubject: profile.user.id,
    installationCredentialDigest: await installationCredentialDigest(
      installationCredential,
    ),
    replacementRecoveryKeyDigest: await recoveryDigest(newRecoveryKey),
  });
  await expect(
    t.mutation(internal.auth.profileRecovery.commitRecoveryAttempt, {
      attemptDigest,
      authSubject: profile.user.id,
      installationCredential,
    }),
  ).resolves.toMatchObject({ activeMacGeneration: 2, authFinalized: false });

  await expect(
    profile.authenticated.mutation(api.sync.dailyUsage, {
      activeMacGeneration: 2,
      installationCredential,
      profileBackfillAnchor: null,
      snapshots: [
        {
          apiEquivalentCost: null,
          correctionReason: null,
          correctionRevision: null,
          coverage: "partial",
          evidenceBasis: "locally-derived",
          observedAt: Date.now(),
          observedTokens: 1,
          provider: "codex",
          rankingDay: new Date().toISOString().slice(0, 10),
          revision: 1,
        },
      ],
    }),
  ).rejects.toThrow("authority-rejected");
  await expect(
    profile.authenticated.mutation(api.tokenmaxxers.updateDisplayName, {
      displayName: "Blocked while recovery is incomplete",
    }),
  ).rejects.toThrow("authority-rejected");
  await expect(
    authFetch(t, "/api/auth/sign-in/username", {
      body: JSON.stringify({
        password: profile.recoveryKey,
        username: profile.touchGrassId,
      }),
      headers: { "content-type": "application/json" },
      method: "POST",
    }),
  ).resolves.toMatchObject({ status: 401 });
  await expect(
    authFetch(t, "/api/auth/touchgrass/recovery/prepare", {
      body: JSON.stringify({
        attemptId: recoveryAttemptId("X"),
        recoveryKey: profile.recoveryKey,
        replacementRecoveryKey: replacementRecoveryKey("Y"),
        touchGrassId: profile.touchGrassId,
      }),
      headers: { "content-type": "application/json" },
      method: "POST",
    }),
  ).resolves.toMatchObject({ status: 401 });
  const authFinalizationClaim = "unfinalized-recovery-test";
  await expect(
    t.mutation(
      internal.auth.profileRecovery.claimRecoveryAuthFinalization,
      {
        attemptDigest,
        authSubject: profile.user.id,
        claim: authFinalizationClaim,
      },
    ),
  ).resolves.toBe(true);
  await expect(
    t.mutation(internal.auth.profileRecovery.finalizeRecoveryAuth, {
      attemptDigest,
      authSubject: profile.user.id,
      claim: authFinalizationClaim,
    }),
  ).resolves.toBe(true);
});

test("Profile recovery credential failures are indistinguishable and rate-limited", async () => {
  vi.stubEnv(
    "BETTER_AUTH_SECRET",
    `${crypto.randomUUID()}${crypto.randomUUID()}`,
  );
  vi.stubEnv("CONVEX_SITE_URL", "https://example.convex.site");

  const t = testBackend();
  const profile = await createRecoverableProfile(t);
  const wrongRecoveryKey = replacementRecoveryKey("Z");
  const failures = await Promise.all(
    [
      {},
      {
        attemptId: recoveryAttemptId("F"),
        recoveryKey: wrongRecoveryKey,
        replacementRecoveryKey: replacementRecoveryKey("F"),
        touchGrassId: "invalid",
      },
      {
        attemptId: recoveryAttemptId("G"),
        recoveryKey: wrongRecoveryKey,
        replacementRecoveryKey: replacementRecoveryKey("G"),
        touchGrassId: "TG-ZZZZ22",
      },
      {
        attemptId: recoveryAttemptId("H"),
        recoveryKey: wrongRecoveryKey,
        replacementRecoveryKey: replacementRecoveryKey("H"),
        touchGrassId: profile.touchGrassId,
      },
      {
        attemptId: "A".repeat(10_000),
        recoveryKey: "R".repeat(10_000),
        replacementRecoveryKey: "S".repeat(10_000),
        touchGrassId: "T".repeat(10_000),
      },
    ].map((body, index) =>
      authFetch(
        t,
        "/api/auth/touchgrass/recovery/prepare",
        {
          body: JSON.stringify(body),
          headers: { "content-type": "application/json" },
          method: "POST",
        },
        `192.0.2.${index + 1}`,
      ),
    ),
  );
  expect(failures.every(({ status }) => status === 401)).toBe(true);
  expect(new Set(await Promise.all(failures.map((failure) => failure.text())))).toHaveLength(1);

  for (let attempt = 1; attempt < touchGrassAuthPolicy.failedRecoveryKey.attempts; attempt += 1) {
    await authFetch(
      t,
      "/api/auth/touchgrass/recovery/prepare",
      {
        body: JSON.stringify({
          attemptId: recoveryAttemptId(String(attempt)),
          recoveryKey: wrongRecoveryKey,
          replacementRecoveryKey: replacementRecoveryKey("W"),
          touchGrassId: profile.touchGrassId,
        }),
        headers: { "content-type": "application/json" },
        method: "POST",
      },
      "203.0.113.200",
    );
  }
  await expect(
    authFetch(
      t,
      "/api/auth/touchgrass/recovery/prepare",
      {
        body: JSON.stringify({
          attemptId: recoveryAttemptId("Q"),
          recoveryKey: profile.recoveryKey,
          replacementRecoveryKey: replacementRecoveryKey("Q"),
          touchGrassId: profile.touchGrassId,
        }),
        headers: { "content-type": "application/json" },
        method: "POST",
      },
      "203.0.113.200",
    ),
  ).resolves.toMatchObject({ status: 429 });
});
