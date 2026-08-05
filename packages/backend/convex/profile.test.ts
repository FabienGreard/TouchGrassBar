/// <reference types="vite/client" />

import betterAuthTest from "@convex-dev/better-auth/test";
import { convexTest } from "convex-test";
import { afterEach, expect, test, vi } from "vitest";

import { api, components } from "./_generated/api";
import schema from "./schema";

const modules = import.meta.glob("./**/*.ts");

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

test("generated credential signup is short-lived, hashed, and session-validating", async () => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date("2026-08-05T00:00:00.000Z"));
  vi.stubEnv(
    "BETTER_AUTH_SECRET",
    `${crypto.randomUUID()}${crypto.randomUUID()}`,
  );
  vi.stubEnv("CONVEX_SITE_URL", "https://example.convex.site");

  const t = convexTest(schema, modules);
  betterAuthTest.register(t);

  const expiredPreparation = await t.fetch(
    "/api/auth/touchgrass/prepare",
    { method: "POST" },
  );
  expect(expiredPreparation.status).toBe(200);
  const expired = await json(expiredPreparation);
  vi.advanceTimersByTime(121_000);

  const recoveryKey = `${crypto.randomUUID()}${crypto.randomUUID()}`;
  const expiredSignup = await t.fetch("/api/auth/sign-up/email", {
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

  const preparation = await t.fetch("/api/auth/touchgrass/prepare", {
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
  const missingProof = await t.fetch("/api/auth/sign-up/email", {
    body: JSON.stringify(signupBody),
    headers: { "content-type": "application/json" },
    method: "POST",
  });
  expect(missingProof.status).toBe(403);

  const signup = await t.fetch("/api/auth/sign-up/email", {
    body: JSON.stringify(signupBody),
    headers: {
      "content-type": "application/json",
      "x-touchgrass-signup-proof": String(prepared.signupProof),
    },
    method: "POST",
  });
  expect(signup.status).toBe(200);
  const signupResult = await json(signup);
  const user = signupResult.user as { id: string };

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

  const signIn = await t.fetch("/api/auth/sign-in/username", {
    body: JSON.stringify({
      password: recoveryKey,
      username: prepared.touchGrassId,
    }),
    headers: { "content-type": "application/json" },
    method: "POST",
  });
  expect(signIn.status).toBe(200);
  const { token: sessionToken } = (await json(signIn)) as { token: string };

  const tokenResponse = await t.fetch("/api/auth/convex/token", {
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

  const signOut = await t.fetch("/api/auth/sign-out", {
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
