import { describe, expect, test } from "vitest";

import {
  assertUsageSnapshot,
  boardKey,
  subtractRankingDays,
} from "../convex/model/values";

describe("ranking invariants", () => {
  test("creates stable namespaced board keys", () => {
    expect(boardKey("combined", 30)).toBe("tokens-v1:combined:30d");
  });

  test("uses UTC calendar arithmetic", () => {
    expect(subtractRankingDays("2026-03-01", 1)).toBe("2026-02-28");
  });

  test("rejects stale-style invalid revisions", () => {
    expect(() =>
      assertUsageSnapshot({
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
      }, "2026-08-03"),
    ).toThrow("revision");
  });
});
