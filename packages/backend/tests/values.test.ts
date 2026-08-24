import { describe, expect, test } from "vitest";

import { BACKEND_POLICY_VERSION, backendPolicy } from "../convex/model/policy";
import { assertUsageSnapshot, boardKey, subtractRankingDays } from "../convex/model/values";

describe("ranking invariants", () => {
  test("creates stable namespaced board keys", () => {
    expect(boardKey("combined", 30)).toBe("tokens-v1:combined:30d");
  });

  test("uses UTC calendar arithmetic", () => {
    expect(subtractRankingDays("2026-03-01", 1)).toBe("2026-02-28");
  });

  test("rejects stale-style invalid revisions", () => {
    expect(() =>
      assertUsageSnapshot(
        {
          apiEquivalentCost: null,
          correctionReason: null,
          correctionRevision: null,
          coverage: "complete",
          evidenceBasis: "locally-derived",
          observedAt: 0,
          observedTokens: 1,
          provider: "codex",
          rankingDay: "2026-08-03",
          revision: 0,
        },
        "2026-08-03",
      ),
    ).toThrow("revision");
  });

  test("keeps readiness limits in one versioned typed policy", () => {
    expect(BACKEND_POLICY_VERSION).toBe("backend-policy-v1");
    expect(backendPolicy).toMatchObject({
      authentication: {
        canaryLifetimeMs: 30 * 60 * 1_000,
      },
      doomerboards: {
        globalResultLimit: 100,
        legacyCompatibilityRows: 640,
        savedTokenmaxxers: 100,
      },
      recovery: {
        failedAttempts: 5,
        failedAttemptWindowMs: 15 * 60 * 1_000,
        successfulTransfers: 3,
        successfulTransferWindowMs: 60 * 60 * 1_000,
      },
      synchronization: {
        maxProfileBackfillSnapshots: 60,
        maxSnapshotsPerRequest: 62,
        rateCapacity: 180,
        ratePerMinute: 60,
      },
    });
  });
});
