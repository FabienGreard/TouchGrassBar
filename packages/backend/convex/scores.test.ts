import { describe, expect, test } from "vitest";

import { rankRows } from "./doomerboards";
import { calculateScore } from "./model/scores";

describe("cost and ranking independence", () => {
  test("a cost-only reprice does not change Token Score or rank", () => {
    const baseRow = {
      apiEquivalentCost: {
        coveragePercent: null,
        micros: 100_000,
        pricingBasis: "openai-api-2026-08-09-v3",
        quality: "local-only" as const,
      },
      observedTokens: 100,
      provider: "codex",
      rankingDay: "2026-08-06",
    };
    const before = calculateScore([baseRow], "codex", 1, "2026-08-06");
    const after = calculateScore(
      [
        {
          ...baseRow,
          apiEquivalentCost: {
            ...baseRow.apiEquivalentCost,
            micros: 250_000,
          },
        },
      ],
      "codex",
      1,
      "2026-08-06",
    );

    const beforeCost = before.apiEquivalentCost;
    const afterCost = after.apiEquivalentCost;
    if (beforeCost === null || afterCost === null) {
      throw new Error("priced rows must produce an API-equivalent cost");
    }
    expect(afterCost.micros).not.toBe(beforeCost.micros);
    expect(after.tokenScore).toBe(before.tokenScore);

    const board = (micros: number) =>
      rankRows([
        {
          apiEquivalentCost: { ...beforeCost, micros: 300_000 },
          displayName: "Higher",
          tokenScore: 200,
          touchGrassId: "TG-HIGHER",
        },
        {
          apiEquivalentCost: { ...beforeCost, micros },
          displayName: "Repriced",
          tokenScore: after.tokenScore,
          touchGrassId: "TG-REPRICED",
        },
        {
          apiEquivalentCost: { ...beforeCost, micros: 50_000 },
          displayName: "Lower",
          tokenScore: 50,
          touchGrassId: "TG-LOWER",
        },
      ]).map(({ rank, tokenScore, touchGrassId }) => ({
        rank,
        tokenScore,
        touchGrassId,
      }));

    expect(board(afterCost.micros)).toEqual(board(beforeCost.micros));
  });

  test("modeled cost metadata survives combined score and board projection", () => {
    const score = calculateScore(
      [
        {
          apiEquivalentCost: {
            coveragePercent: null,
            micros: 1_000_000,
            pricingBasis: "openai-api-2026-08-09-v3",
            quality: "reconciled" as const,
          },
          observedTokens: 100,
          provider: "codex",
          rankingDay: "2026-08-06",
        },
        {
          apiEquivalentCost: {
            coveragePercent: 50,
            micros: 2_000_000,
            pricingBasis: "anthropic-standard-2026-08-07-v1",
            quality: "modeled" as const,
          },
          observedTokens: 300,
          provider: "claude",
          rankingDay: "2026-08-06",
        },
        {
          apiEquivalentCost: null,
          observedTokens: 100,
          provider: "codex",
          rankingDay: "2026-08-06",
        },
      ],
      "combined",
      1,
      "2026-08-06",
    );

    expect(score).toEqual({
      apiEquivalentCost: {
        coveragePercent: 50,
        micros: 3_000_000,
        pricingBasis:
          "anthropic-standard-2026-08-07-v1 + openai-api-2026-08-09-v3",
        quality: "modeled",
      },
      tokenScore: 500,
    });
    const projectedCost = score.apiEquivalentCost;
    if (!projectedCost) {
      throw new Error("priced rows must keep the complete cost object");
    }
    expect(
      rankRows([
        {
          apiEquivalentCost: projectedCost,
          displayName: "Modeled",
          tokenScore: score.tokenScore,
          touchGrassId: "TG-MODELED",
        },
      ]),
    ).toEqual([
      {
        apiEquivalentCost: score.apiEquivalentCost,
        displayName: "Modeled",
        rank: 1,
        tokenScore: 500,
        touchGrassId: "TG-MODELED",
      },
    ]);
  });
});
