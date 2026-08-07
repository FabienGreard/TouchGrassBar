import type { UsagePeriods } from "@touchgrass/contracts";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, test } from "vitest";

import { UsageOverview } from "@/components/panel/usage-overview";

function unavailablePeriods(todayScanStatus: "indexing" | "unavailable") {
  return {
    scanStatus: todayScanStatus,
    sevenDayScanStatus: "unavailable",
    sevenDays: { availability: "unavailable" },
    thirtyDayScanStatus: "unavailable",
    thirtyDays: { availability: "unavailable" },
    today: { availability: "unavailable" },
    todayScanStatus,
  } as const satisfies UsagePeriods;
}

describe("usage overview", () => {
  test("shows indexing when a period has no evidence while its scan continues", () => {
    const markup = renderToStaticMarkup(
      <UsageOverview usage={unavailablePeriods("indexing")} />,
    );

    expect(markup.match(/Indexing…/g)).toHaveLength(1);
    expect(markup.match(/Not observed/g)).toHaveLength(2);
  });

  test("keeps known usage and cost when another provider is still indexing", () => {
    const usage = unavailablePeriods("indexing");
    const current: UsagePeriods = {
      ...usage,
      today: {
        apiEquivalentCostBasis: "openai-fixture-v1",
        apiEquivalentCostCoveragePercent: null,
        apiEquivalentCostQuality: "reconciled",
        apiEquivalentCostUsd: 2,
        availability: "current",
        coverage: "complete",
        evidenceBasis: "provider-reported",
        observedAt: "2026-08-08T00:00:00.000Z",
        observedTokens: 100,
        trendPercent: null,
        trendPreviousTokens: null,
      },
    };

    const markup = renderToStaticMarkup(<UsageOverview usage={current} />);

    expect(markup).toContain(">≈ $2.00</span>");
    expect(markup).toContain(">100</strong>");
    expect(markup).not.toContain("Indexing…");
  });
});
