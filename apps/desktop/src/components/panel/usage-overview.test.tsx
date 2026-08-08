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
  test("shows the normalized top model as usage context", () => {
    const markup = renderToStaticMarkup(
      <UsageOverview
        topModelUsage={{ model: "GPT 5.6 Sol", observedTokens: 100 }}
        usage={unavailablePeriods("unavailable")}
      />,
    );

    expect(markup).toContain(">Usage</h2>");
    expect(markup).toContain("Most used");
    expect(markup).toContain(" · GPT 5.6 Sol</span>");
    expect(markup).not.toContain("Observed tokens");
    expect(markup).not.toContain("API equivalent</small>");
  });

  test("shows only a dash when no recognized model is available", () => {
    const markup = renderToStaticMarkup(
      <UsageOverview usage={unavailablePeriods("unavailable")} />,
    );

    expect(markup).toContain(">Usage</h2>");
    expect(markup).toContain(">—</small>");
    expect(markup).not.toContain("Most used");
  });

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
