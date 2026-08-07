/// <reference types="vite/client" />

import betterAuthTest from "@convex-dev/better-auth/test";
import rateLimiterTest from "@convex-dev/rate-limiter/test";
import { convexTest } from "convex-test";
import { afterEach, expect, test, vi } from "vitest";

import { api, components, internal } from "./_generated/api";
import { createAuthWithRequestIp } from "./auth";
import { touchGrassAuthPolicy } from "./model/rateLimits";
import schema from "./schema";

const modules = import.meta.glob("./**/*.ts");
const DEFAULT_PLATFORM_IP = "203.0.113.10";

function testBackend() {
  const t = convexTest(schema, modules);
  betterAuthTest.register(t);
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

afterEach(() => {
  vi.unstubAllEnvs();
  vi.useRealTimers();
});

test("Recovery Key signup is short-lived, hashed, and session-validating", async () => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date("2026-08-05T00:00:00.000Z"));
  vi.stubEnv(
    "BETTER_AUTH_SECRET",
    `${crypto.randomUUID()}${crypto.randomUUID()}`,
  );
  vi.stubEnv("CONVEX_SITE_URL", "https://example.convex.site");

  const t = testBackend();

  const expiredPreparation = await authFetch(
    t,
    "/api/auth/touchgrass/prepare",
    { method: "POST" },
  );
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
  expect(concurrentSignups.map(({ status }) => status).sort()).toEqual([
    200, 403,
  ]);
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
    }),
  ).resolves.toEqual({
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
    }),
  ).rejects.toThrow("Unauthenticated");
});

test("failed Recovery Keys use independent opaque limits", async () => {
  vi.stubEnv(
    "BETTER_AUTH_SECRET",
    `${crypto.randomUUID()}${crypto.randomUUID()}`,
  );
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
    await signIn(
      `TG-AAAA${21 + attempts}`,
      wrongRecoveryKey,
      "192.0.2.1",
      "198.18.0.100",
    ),
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
      await signIn(
        byIdCredential.touchGrassId,
        wrongRecoveryKey,
        `198.51.100.${attempt + 1}`,
      ),
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
      t.mutation(
        internal.auth.credentialAttempts.reserveCredentialAttempt,
        keys,
      ),
    ),
  );
  const reservationIds = admitted.filter(
    (reservationId) => reservationId !== null,
  );
  expect(reservationIds).toHaveLength(attempts);

  await t.mutation(
    internal.auth.credentialAttempts.finalizeCredentialAttempt,
    { outcome: "success", reservationId: reservationIds[0]! },
  );
  const replacementId = await t.mutation(
    internal.auth.credentialAttempts.reserveCredentialAttempt,
    keys,
  );
  expect(replacementId).not.toBeNull();
  if (!replacementId) throw new Error("Replacement reservation is missing");

  for (const reservationId of [
    ...reservationIds.slice(1, attempts),
    replacementId,
  ]) {
    await t.mutation(
      internal.auth.credentialAttempts.finalizeCredentialAttempt,
      { outcome: "failure", reservationId },
    );
  }
  await expect(
    t.mutation(
      internal.auth.credentialAttempts.reserveCredentialAttempt,
      keys,
    ),
  ).resolves.toBeNull();

  vi.advanceTimersByTime(touchGrassAuthPolicy.failedRecoveryKey.windowMs + 1);
  await expect(
    t.mutation(
      internal.auth.credentialAttempts.reserveCredentialAttempt,
      keys,
    ),
  ).resolves.not.toBeNull();
});

test("Profile preparation uses the typed IP boundary and reset window", async () => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date("2026-08-05T00:00:00.000Z"));
  vi.stubEnv(
    "BETTER_AUTH_SECRET",
    `${crypto.randomUUID()}${crypto.randomUUID()}`,
  );
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
