import { describe, expect, test } from "vitest";

import {
  buildBackendReadinessEvidence,
  type BackendReadinessInput,
} from "./backend-readiness/evidence";
import {
  deploymentBindingMatches,
  renderDeploymentBinding,
} from "./backend-readiness/deployment-binding";
import { productionConfiguration } from "./backend-readiness/production-configuration";
import {
  AuthorityRejectedError,
  runAuthenticatedCanary,
  type CanaryPort,
} from "./backend-readiness/canary";
import { productionHealthReceipt } from "./backend-readiness/health";
import {
  preflightMatchesSource,
  type BackendReadinessPreflight,
} from "./backend-readiness/preflight";
import { collectFunctionLogs } from "./backend-readiness/production-health-port";
import { readBoundedResponseText } from "./backend-readiness/response-body";

const passedCheck = {
  completedAt: "2026-08-24T13:00:00.000Z",
  status: "passed" as const,
};

function readyInput(): BackendReadinessInput {
  const binding = {
    boardKeyVersion: "tokens-v1",
    commit: "a".repeat(40),
    lockHash: "b".repeat(64),
    policyVersion: "backend-policy-v1",
    schemaHash: "c".repeat(64),
  };
  return {
    checks: {
      authenticatedCanary: passedCheck,
      automatedSuite: passedCheck,
      migrationRehearsal: passedCheck,
      productionHealth: passedCheck,
    },
    deployment: {
      kind: "production",
      name: "exact-production-deployment",
      url: "https://exact-production-deployment.convex.cloud",
    },
    generatedAt: "2026-08-24T13:01:00.000Z",
    runtimeBinding: binding,
    sourceBinding: binding,
  };
}

describe("Backend Readiness Evidence", () => {
  test("represents exact complete production evidence as canary-ready only", () => {
    expect(buildBackendReadinessEvidence(readyInput())).toEqual({
      checks: {
        authenticatedCanary: passedCheck,
        automatedSuite: passedCheck,
        migrationRehearsal: passedCheck,
        productionHealth: passedCheck,
      },
      contractVersion: 1,
      deployment: {
        kind: "production",
        name: "exact-production-deployment",
        url: "https://exact-production-deployment.convex.cloud",
      },
      generatedAt: "2026-08-24T13:01:00.000Z",
      productionReadiness: "not-ready",
      readiness: "canary-ready",
      runtimeBinding: readyInput().runtimeBinding,
      sourceBinding: readyInput().sourceBinding,
      staleReasons: [],
      trafficEvidence: "canary-only",
    });
  });

  test.each(["failed", "skipped"] as const)(
    "fails closed when one mandatory check is %s",
    (status) => {
      const input = readyInput();
      input.checks.productionHealth = {
        ...passedCheck,
        status,
      };

      expect(buildBackendReadinessEvidence(input).readiness).toBe("not-ready");
    },
  );

  test("fails closed when a mandatory check is missing at runtime", () => {
    const input = readyInput() as Omit<BackendReadinessInput, "checks"> & {
      checks: Partial<BackendReadinessInput["checks"]>;
    };
    delete input.checks.authenticatedCanary;

    expect(buildBackendReadinessEvidence(input as BackendReadinessInput)).toMatchObject({
      productionReadiness: "not-ready",
      readiness: "not-ready",
    });
  });

  test("marks evidence stale after any relevant binding changes", () => {
    const input = readyInput();
    input.runtimeBinding = {
      ...input.runtimeBinding,
      schemaHash: "d".repeat(64),
    };

    expect(buildBackendReadinessEvidence(input)).toMatchObject({
      readiness: "not-ready",
      staleReasons: ["schemaHash"],
    });
  });
});

describe("production deployment guard", () => {
  test("renders the source binding into the deployed bundle", () => {
    const binding = readyInput().sourceBinding;
    const rendered = renderDeploymentBinding(binding, "exact-production-deployment");

    expect(rendered).toContain(`commit: "${binding.commit}"`);
    expect(rendered).toContain('productionDeployment: "exact-production-deployment"');
    expect(rendered).not.toContain("private-key");
  });

  test("rejects a tampered generated deployment binding", () => {
    const binding = readyInput().sourceBinding;
    const rendered = renderDeploymentBinding(binding, "exact-production-deployment");

    expect(deploymentBindingMatches(rendered, binding, "exact-production-deployment")).toBe(true);
    expect(
      deploymentBindingMatches(
        rendered.replace(binding.commit, "f".repeat(40)),
        binding,
        "exact-production-deployment",
      ),
    ).toBe(false);
  });

  test("accepts only one exact production deployment binding", () => {
    const configuration = productionConfiguration({
      CONVEX_DEPLOY_KEY: "prod:exact-production-deployment|private-key",
      TOUCHGRASS_PRODUCTION_CONVEX_URL:
        "https://exact-production-deployment.eu-west-1.convex.cloud",
      TOUCHGRASS_PRODUCTION_DEPLOYMENT: "exact-production-deployment",
      TOUCHGRASS_PRODUCTION_SITE_URL: "https://exact-production-deployment.eu-west-1.convex.site",
    });

    expect(configuration).toEqual({
      adminKey: "prod:exact-production-deployment|private-key",
      deployment: {
        kind: "production",
        name: "exact-production-deployment",
        siteUrl: "https://exact-production-deployment.eu-west-1.convex.site",
        url: "https://exact-production-deployment.eu-west-1.convex.cloud",
      },
    });
  });

  test.each([
    ["dev key", "dev:exact-production-deployment|private-key"],
    ["preview key", "preview:exact-production-deployment|private-key"],
    ["other deployment", "prod:other-production-deployment|private-key"],
  ])("rejects a %s", (_label, deployKey) => {
    expect(() =>
      productionConfiguration({
        CONVEX_DEPLOY_KEY: deployKey,
        TOUCHGRASS_PRODUCTION_CONVEX_URL: "https://exact-production-deployment.convex.cloud",
        TOUCHGRASS_PRODUCTION_DEPLOYMENT: "exact-production-deployment",
        TOUCHGRASS_PRODUCTION_SITE_URL: "https://exact-production-deployment.convex.site",
      }),
    ).toThrow("exact production deployment");
  });

  test.each([
    ["other host", "https://other-production-deployment.convex.cloud"],
    ["embedded credentials", "https://user:password@exact-production-deployment.convex.cloud"],
    ["custom port", "https://exact-production-deployment.convex.cloud:444"],
    ["path", "https://exact-production-deployment.convex.cloud/functions"],
  ])("rejects a production URL with an %s", (_label, convexUrl) => {
    expect(() =>
      productionConfiguration({
        CONVEX_DEPLOY_KEY: "prod:exact-production-deployment|private-key",
        TOUCHGRASS_PRODUCTION_CONVEX_URL: convexUrl,
        TOUCHGRASS_PRODUCTION_DEPLOYMENT: "exact-production-deployment",
        TOUCHGRASS_PRODUCTION_SITE_URL: "https://exact-production-deployment.convex.site",
      }),
    ).toThrow("exact production deployment");
  });
});

describe("local readiness preflight guard", () => {
  test("accepts only passed checks for the exact source binding", () => {
    const sourceBinding = readyInput().sourceBinding;
    const preflight = {
      checks: {
        automatedSuite: passedCheck,
        migrationRehearsal: passedCheck,
      },
      contractVersion: 1,
      generatedAt: "2026-08-24T13:00:00.000Z",
      sourceBinding,
    } satisfies BackendReadinessPreflight;

    expect(preflightMatchesSource(preflight, sourceBinding)).toBe(true);
    expect(
      preflightMatchesSource(
        {
          ...preflight,
          checks: {
            ...preflight.checks,
            migrationRehearsal: { ...passedCheck, status: "skipped" },
          },
        },
        sourceBinding,
      ),
    ).toBe(false);
    expect(preflightMatchesSource(preflight, { ...sourceBinding, commit: "d".repeat(40) })).toBe(
      false,
    );
  });
});

describe("authenticated production canary", () => {
  test("proves the full transfer flow and returns sanitized aggregate evidence", async () => {
    let signInCount = 0;
    let syncCount = 0;
    const synchronizationJwts: string[] = [];
    const port: CanaryPort = {
      cleanup: async () => ({
        aggregateEntriesRemoved: 9,
        appRecordsRemoved: 20,
        authRecordsRemoved: 3,
        cleanupComplete: true,
        rateLimitKeysReset: 3,
      }),
      commitRecovery: async () => ({ activeMacGeneration: 2, authFinalized: true }),
      ensureProfile: async () => ({ activeMacGeneration: 1 }),
      exchangeSession: async (session) => `${session}-jwt`,
      globalRows: async () => [{ touchGrassId: "TG-AAAAAA" }],
      myTokenmaxxerRows: async () => ({ rows: [], savedTokenmaxxerCount: 0 }),
      prepareProfile: async () => ({
        signupProof: "private-signup-proof",
        touchGrassId: "TG-AAAAAA",
      }),
      prepareRecovery: async () => ({ recoveryProof: "private-recovery-proof" }),
      registerCanary: async () => undefined,
      signIn: async () => {
        signInCount += 1;
        return signInCount === 1 ? "private-old-session" : "private-new-session";
      },
      signUp: async () => undefined,
      syncUsage: async (args) => {
        synchronizationJwts.push(args.jwt);
        syncCount += 1;
        if (syncCount === 3) throw new AuthorityRejectedError();
        return [{ outcome: syncCount === 2 ? "idempotent" : "committed" }];
      },
    };

    const result = await runAuthenticatedCanary(port, {
      now: () => Date.parse("2026-08-24T13:00:00.000Z"),
      randomCredential: (length) => "private-secret".padEnd(length, "A"),
    });

    expect(result).toEqual({
      checks: {
        cleanup: true,
        generatedCredentials: true,
        globalRead: true,
        identicalRetry: true,
        myTokenmaxxersRead: true,
        newMacSync: true,
        oldMacRejected: true,
        sessionExchange: true,
        synchronization: true,
        transfer: true,
      },
      cleanup: {
        aggregateEntriesRemoved: 9,
        appRecordsRemoved: 20,
        authRecordsRemoved: 3,
        rateLimitKeysReset: 3,
      },
      completedAt: "2026-08-24T13:00:00.000Z",
      startedAt: "2026-08-24T13:00:00.000Z",
    });
    expect(synchronizationJwts).toEqual([
      "private-old-session-jwt",
      "private-old-session-jwt",
      "private-new-session-jwt",
      "private-new-session-jwt",
    ]);
    const serialized = JSON.stringify(result);
    for (const privateValue of [
      "TG-AAAAAA",
      "private-signup-proof",
      "private-recovery-proof",
      "private-old-session",
      "private-new-session",
      "private-secret",
    ]) {
      expect(serialized).not.toContain(privateValue);
    }
  });

  test("fails when disposable Profile cleanup is incomplete", async () => {
    const port: CanaryPort = {
      cleanup: async () => ({
        aggregateEntriesRemoved: 0,
        appRecordsRemoved: 0,
        authRecordsRemoved: 0,
        cleanupComplete: false,
        rateLimitKeysReset: 0,
      }),
      commitRecovery: async () => ({ activeMacGeneration: 2, authFinalized: true }),
      ensureProfile: async () => ({ activeMacGeneration: 1 }),
      exchangeSession: async (session) => session,
      globalRows: async () => [{ touchGrassId: "TG-AAAAAA" }],
      myTokenmaxxerRows: async () => ({ rows: [], savedTokenmaxxerCount: 0 }),
      prepareProfile: async () => ({ signupProof: "proof", touchGrassId: "TG-AAAAAA" }),
      prepareRecovery: async () => ({ recoveryProof: "proof" }),
      registerCanary: async () => undefined,
      signIn: async () => "session",
      signUp: async () => undefined,
      syncUsage: async () => [{ outcome: "committed" }],
    };

    await expect(
      runAuthenticatedCanary(port, {
        now: () => Date.parse("2026-08-24T13:00:00.000Z"),
        randomCredential: (length) => "A".repeat(length),
      }),
    ).rejects.toThrow("cleanup failed");
  });

  test("uses a fresh UTC Ranking Day when the transfer crosses midnight", async () => {
    let syncCount = 0;
    const rankingDays: string[] = [];
    const globalRankingDays: string[] = [];
    const port: CanaryPort = {
      cleanup: async () => ({
        aggregateEntriesRemoved: 9,
        appRecordsRemoved: 10,
        authRecordsRemoved: 3,
        cleanupComplete: true,
        rateLimitKeysReset: 3,
      }),
      commitRecovery: async () => ({ activeMacGeneration: 2, authFinalized: true }),
      ensureProfile: async () => ({ activeMacGeneration: 1 }),
      exchangeSession: async (session) => `${session}-jwt`,
      globalRows: async (args) => {
        globalRankingDays.push(args.rankingDay);
        return [{ touchGrassId: "TG-AAAAAA" }];
      },
      myTokenmaxxerRows: async () => ({ rows: [], savedTokenmaxxerCount: 0 }),
      prepareProfile: async () => ({ signupProof: "proof", touchGrassId: "TG-AAAAAA" }),
      prepareRecovery: async () => ({ recoveryProof: "proof" }),
      registerCanary: async () => undefined,
      signIn: async () => "session",
      signUp: async () => undefined,
      syncUsage: async (args) => {
        rankingDays.push(args.rankingDay);
        syncCount += 1;
        if (syncCount === 3) throw new AuthorityRejectedError();
        return [{ outcome: syncCount === 2 ? "idempotent" : "committed" }];
      },
    };
    const times = [
      "2026-08-24T23:59:58.000Z",
      "2026-08-24T23:59:59.000Z",
      "2026-08-25T00:00:01.000Z",
      "2026-08-25T00:00:02.000Z",
    ].map(Date.parse);

    await runAuthenticatedCanary(port, {
      now: () => times.shift() ?? Date.parse("2026-08-25T00:00:02.000Z"),
      randomCredential: (length) => "A".repeat(length),
    });

    expect(rankingDays).toEqual(["2026-08-24", "2026-08-24", "2026-08-25", "2026-08-25"]);
    expect(globalRankingDays).toEqual(["2026-08-24", "2026-08-25"]);
  });
});

describe("production health receipt", () => {
  test("cancels a streamed response as soon as its byte limit is exceeded", async () => {
    let cancelled = false;
    const body = new ReadableStream<Uint8Array>({
      cancel: () => {
        cancelled = true;
      },
      start(controller) {
        controller.enqueue(new TextEncoder().encode("1234"));
        controller.enqueue(new TextEncoder().encode("5678"));
      },
    });

    await expect(
      readBoundedResponseText(new Response(body), 7, "Bounded response exceeded"),
    ).rejects.toThrow("Bounded response exceeded");
    expect(cancelled).toBe(true);
  });

  test("collects every bounded log page through the complete canary window", async () => {
    const cursors: number[] = [];
    const logs = await collectFunctionLogs(
      async (cursor) => {
        cursors.push(cursor);
        return {
          byteLength: 100,
          value: {
            entries: [
              {
                error: cursor === 1_000 ? "authority-rejected" : null,
                identifier: "sync:dailyUsage",
                kind: "Completion",
                timestamp: cursor / 1_000,
              },
            ],
            newCursor: cursor + 1_000,
          },
        };
      },
      2_000,
      3_000,
    );

    expect(cursors).toEqual([1_000, 2_000]);
    expect(logs).toHaveLength(2);
  });

  test("fails closed when a production log cursor does not advance", async () => {
    await expect(
      collectFunctionLogs(
        async (cursor) => ({
          byteLength: 10,
          value: { entries: [], newCursor: cursor },
        }),
        2_000,
        3_000,
      ),
    ).rejects.toThrow("did not advance");
  });

  test("fails closed when a production error log entry is malformed", async () => {
    await expect(
      collectFunctionLogs(
        async () => ({
          byteLength: 10,
          value: {
            entries: [{ error: 42, identifier: "sync:dailyUsage", kind: "Completion" }],
            newCursor: 3_000,
          },
        }),
        2_000,
        3_000,
      ),
    ).rejects.toThrow("response is invalid");
  });

  test("accepts count-only invariants and the one expected old-Mac rejection", () => {
    expect(
      productionHealthReceipt({
        completedAt: "2026-08-24T13:00:10.000Z",
        expectedDeploymentName: "exact-production-deployment",
        inspection: {
          canaryResidue: { markers: 0 },
          componentChecks: {
            betterAuth: true,
            doomerboard: true,
            migrations: true,
            rateLimiter: true,
          },
          deviceMigration: { devices: 20, missingCompletionFields: 0 },
          doomerboardInvariant: {
            aggregateEntries: 180,
            extraEntries: 0,
            invalidEntries: 0,
            mismatchedEntries: 0,
            missingEntries: 0,
            publicScores: 180,
          },
          requiredEnvironment: {
            backendBinding: true,
            betterAuthSecret: true,
            productionDeployment: true,
          },
          productionDeployment: "exact-production-deployment",
        },
        logs: [
          {
            error: "ConvexError: { code: authority-rejected }",
            identifier: "sync:dailyUsage",
            kind: "Completion",
            timestamp: Date.parse("2026-08-24T13:00:05.000Z") / 1_000,
          },
        ],
        window: {
          completedAtMs: Date.parse("2026-08-24T13:00:09.000Z"),
          startedAtMs: Date.parse("2026-08-24T13:00:00.000Z"),
        },
      }),
    ).toEqual({
      completedAt: "2026-08-24T13:00:10.000Z",
      counts: {
        aggregateEntries: 180,
        canaryMarkers: 0,
        componentsPassed: 4,
        devices: 20,
        expectedAuthorityRejections: 1,
        publicScores: 180,
        unhandledErrors: 0,
      },
      status: "passed",
    });
  });

  test("fails closed on an unhandled backend error", () => {
    const receipt = productionHealthReceipt({
      completedAt: "2026-08-24T13:00:10.000Z",
      expectedDeploymentName: "exact-production-deployment",
      inspection: {
        canaryResidue: { markers: 0 },
        componentChecks: {
          betterAuth: true,
          doomerboard: true,
          migrations: true,
          rateLimiter: true,
        },
        deviceMigration: { devices: 0, missingCompletionFields: 0 },
        doomerboardInvariant: {
          aggregateEntries: 0,
          extraEntries: 0,
          invalidEntries: 0,
          mismatchedEntries: 0,
          missingEntries: 0,
          publicScores: 0,
        },
        requiredEnvironment: {
          backendBinding: true,
          betterAuthSecret: true,
          productionDeployment: true,
        },
        productionDeployment: "exact-production-deployment",
      },
      logs: [
        {
          error: "Unhandled failure",
          identifier: "sync:dailyUsage",
          kind: "Completion",
          timestamp: Date.parse("2026-08-24T13:00:05.000Z") / 1_000,
        },
      ],
      window: {
        completedAtMs: Date.parse("2026-08-24T13:00:09.000Z"),
        startedAtMs: Date.parse("2026-08-24T13:00:00.000Z"),
      },
    });

    expect(receipt).toMatchObject({ status: "failed" });
  });

  test("fails closed when an earlier canary marker remains", () => {
    const receipt = productionHealthReceipt({
      completedAt: "2026-08-24T13:00:10.000Z",
      expectedDeploymentName: "exact-production-deployment",
      inspection: {
        canaryResidue: { markers: 1 },
        componentChecks: {
          betterAuth: true,
          doomerboard: true,
          migrations: true,
          rateLimiter: true,
        },
        deviceMigration: { devices: 0, missingCompletionFields: 0 },
        doomerboardInvariant: {
          aggregateEntries: 0,
          extraEntries: 0,
          invalidEntries: 0,
          mismatchedEntries: 0,
          missingEntries: 0,
          publicScores: 0,
        },
        requiredEnvironment: {
          backendBinding: true,
          betterAuthSecret: true,
          productionDeployment: true,
        },
        productionDeployment: "exact-production-deployment",
      },
      logs: [
        {
          error: "ConvexError: { code: authority-rejected }",
          identifier: "sync:dailyUsage",
          kind: "Completion",
          timestamp: Date.parse("2026-08-24T13:00:05.000Z") / 1_000,
        },
      ],
      window: {
        completedAtMs: Date.parse("2026-08-24T13:00:09.000Z"),
        startedAtMs: Date.parse("2026-08-24T13:00:00.000Z"),
      },
    });

    expect(receipt).toMatchObject({ counts: { canaryMarkers: 1 }, status: "failed" });
  });
});
