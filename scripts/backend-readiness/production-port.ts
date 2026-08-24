import { ConvexHttpClient } from "convex/browser";

import { api, internal } from "../../packages/backend/convex/_generated/api";
import { adminClient } from "./admin-client";
import { AuthorityRejectedError, type CanaryPort } from "./canary";
import type { ProductionConfiguration } from "./production-configuration";
import { readBoundedResponseText } from "./response-body";

const MAX_AUTH_RESPONSE_BYTES = 64 * 1_024;

function record(value: unknown): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new Error("Authenticated canary received an invalid response");
  }
  return value as Record<string, unknown>;
}

function stringField(value: unknown, name: string) {
  const field = record(value)[name];
  if (typeof field !== "string" || field.length === 0) {
    throw new Error("Authenticated canary received an invalid response");
  }
  return field;
}

function numberField(value: unknown, name: string) {
  const field = record(value)[name];
  if (!Number.isSafeInteger(field)) {
    throw new Error("Authenticated canary received an invalid response");
  }
  return field as number;
}

function booleanField(value: unknown, name: string) {
  const field = record(value)[name];
  if (typeof field !== "boolean") {
    throw new Error("Authenticated canary received an invalid response");
  }
  return field;
}

async function authRequest(
  siteUrl: string,
  path: string,
  init: {
    body?: Record<string, unknown>;
    headers?: Record<string, string>;
    method?: "GET" | "POST";
  },
) {
  const response = await fetch(`${siteUrl}${path}`, {
    ...(init.body === undefined ? {} : { body: JSON.stringify(init.body) }),
    headers: {
      ...(init.body === undefined ? {} : { "content-type": "application/json" }),
      ...init.headers,
    },
    method: init.method ?? "POST",
  });
  if (!response.ok) {
    await response.body?.cancel();
    throw new Error("Authenticated canary request failed");
  }
  const { text: body } = await readBoundedResponseText(
    response,
    MAX_AUTH_RESPONSE_BYTES,
    "Authenticated canary request failed",
  );
  if (body.length === 0) return {};
  try {
    return JSON.parse(body) as unknown;
  } catch {
    throw new Error("Authenticated canary received an invalid response");
  }
}

function userClient(url: string, jwt: string) {
  const client = new ConvexHttpClient(url);
  client.setAuth(jwt);
  return client;
}

function authorityWasRejected(error: unknown) {
  if (typeof error !== "object" || error === null || !("data" in error)) return false;
  const data = (error as { data?: unknown }).data;
  return (
    typeof data === "object" &&
    data !== null &&
    "code" in data &&
    (data as { code?: unknown }).code === "authority-rejected"
  );
}

export function productionCanaryPort(configuration: ProductionConfiguration): CanaryPort {
  const { deployment } = configuration;
  const administrator = adminClient(deployment.url, configuration.adminKey);

  return {
    cleanup: (args) => administrator.mutation(internal.internal.readiness.cleanupCanary, args),
    commitRecovery: async (args) => {
      const response = await authRequest(
        deployment.siteUrl,
        "/api/auth/touchgrass/recovery/commit",
        {
          body: args,
        },
      );
      return {
        activeMacGeneration: numberField(response, "activeMacGeneration"),
        authFinalized: booleanField(response, "authFinalized"),
      };
    },
    ensureProfile: (args) =>
      userClient(deployment.url, args.jwt).mutation(api.tokenmaxxers.ensureProfile, {
        displayName: args.displayName,
        expectedTouchGrassId: args.touchGrassId,
        installationCredential: args.installationCredential,
      }),
    exchangeSession: async (session) => {
      const response = await authRequest(deployment.siteUrl, "/api/auth/convex/token", {
        headers: { authorization: `Bearer ${session}` },
        method: "GET",
      });
      return stringField(response, "token");
    },
    globalRows: async (args) =>
      userClient(deployment.url, args.jwt).query(api.doomerboards.currentGlobal, {
        limit: 100,
        rankingDay: args.rankingDay,
        scope: "combined",
        windowDays: 1,
      }),
    myTokenmaxxerRows: (args) =>
      userClient(deployment.url, args.jwt).query(api.doomerboards.currentMyTokenmaxxers, {
        rankingDay: args.rankingDay,
        scope: "combined",
        windowDays: 1,
      }),
    prepareProfile: async () => {
      const response = await authRequest(deployment.siteUrl, "/api/auth/touchgrass/prepare", {
        body: {},
      });
      return {
        signupProof: stringField(response, "signupProof"),
        touchGrassId: stringField(response, "touchGrassId"),
      };
    },
    prepareRecovery: async (args) => {
      const response = await authRequest(
        deployment.siteUrl,
        "/api/auth/touchgrass/recovery/prepare",
        { body: args },
      );
      return { recoveryProof: stringField(response, "recoveryProof") };
    },
    registerCanary: (args) =>
      administrator
        .mutation(internal.internal.readiness.registerCanary, args)
        .then(() => undefined),
    signIn: async (args) => {
      const response = await authRequest(deployment.siteUrl, "/api/auth/sign-in/username", {
        body: {
          password: args.recoveryKey,
          username: args.touchGrassId,
        },
      });
      return stringField(response, "token");
    },
    signUp: async (args) => {
      await authRequest(deployment.siteUrl, "/api/auth/sign-up/email", {
        body: {
          email: `${args.touchGrassId.toLowerCase()}@profile.touchgrass.invalid`,
          name: args.displayName,
          password: args.recoveryKey,
          username: args.touchGrassId,
        },
        headers: { "x-touchgrass-signup-proof": args.signupProof },
      });
    },
    syncUsage: async (args) => {
      try {
        return await userClient(deployment.url, args.jwt).mutation(api.sync.dailyUsage, {
          activeMacGeneration: args.activeMacGeneration,
          installationCredential: args.installationCredential,
          profileBackfillAnchor: null,
          snapshots: [
            {
              apiEquivalentCost: null,
              correctionReason: null,
              correctionRevision: null,
              coverage: "complete",
              evidenceBasis: "locally-derived",
              observedAt: args.observedAt,
              observedTokens: args.observedTokens,
              provider: "codex",
              rankingDay: args.rankingDay,
              revision: args.revision,
            },
          ],
        });
      } catch (error) {
        if (authorityWasRejected(error)) throw new AuthorityRejectedError();
        // oxlint-disable-next-line preserve-caught-error -- Convex errors can contain private canary values.
        throw new Error("Authenticated canary synchronization failed");
      }
    },
  };
}
