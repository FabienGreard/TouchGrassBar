import { describe, expect, test } from "vitest";

import { rankRows } from "./doomerboards";
import { calculateScore } from "./model/scores";

describe("cost and ranking independence", () => {
  test("a cost-only reprice does not change Token Score or rank", () => {
    const baseRow = {
      apiEquivalentCost: {
        coveragePercent: null,
        micros: 100_000,
        pricingBasis: "openai-standard-2026-08-06-v1",
        quality: "local-only" as const,
      },
      costIsComplete: true,
      observedTokens: 100,
      provider: "codex",
      rankingDay: "2026-08-06",
    };
    const before = calculateScore(
      [{ ...baseRow, apiEquivalentCostMicros: 100_000 }],
      "codex",
      1,
      "2026-08-06",
    );
    const after = calculateScore(
      [
        {
          ...baseRow,
          apiEquivalentCost: {
            ...baseRow.apiEquivalentCost,
            micros: 250_000,
          },
          apiEquivalentCostMicros: 250_000,
        },
      ],
      "codex",
      1,
      "2026-08-06",
    );

    const beforeCost = before.apiEquivalentCostMicros;
    const afterCost = after.apiEquivalentCostMicros;
    if (beforeCost === undefined || afterCost === undefined) {
      throw new Error("priced rows must produce an API-equivalent cost");
    }
    expect(afterCost).not.toBe(beforeCost);
    expect(after.tokenScore).toBe(before.tokenScore);

    const board = (apiEquivalentCostMicros: number) =>
      rankRows([
        {
          apiEquivalentCostMicros: 300_000,
          displayName: "Higher",
          tokenScore: 200,
          touchGrassId: "TG-HIGHER",
        },
        {
          apiEquivalentCostMicros,
          displayName: "Repriced",
          tokenScore: after.tokenScore,
          touchGrassId: "TG-REPRICED",
        },
        {
          apiEquivalentCostMicros: 50_000,
          displayName: "Lower",
          tokenScore: 50,
          touchGrassId: "TG-LOWER",
        },
      ]).map(({ rank, tokenScore, touchGrassId }) => ({
        rank,
        tokenScore,
        touchGrassId,
      }));

    expect(board(afterCost)).toEqual(board(beforeCost));
  });

  test("modeled cost metadata survives combined score and board projection", () => {
    const score = calculateScore(
      [
        {
          apiEquivalentCost: {
            coveragePercent: null,
            micros: 1_000_000,
            pricingBasis: "openai-standard-2026-08-06-v1",
            quality: "reconciled" as const,
          },
          apiEquivalentCostMicros: 1_000_000,
          costIsComplete: true,
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
          apiEquivalentCostMicros: 2_000_000,
          costIsComplete: false,
          observedTokens: 300,
          provider: "claude",
          rankingDay: "2026-08-06",
        },
        {
          costIsComplete: false,
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
          "anthropic-standard-2026-08-07-v1 + openai-standard-2026-08-06-v1",
        quality: "modeled",
      },
      apiEquivalentCostMicros: 3_000_000,
      tokenScore: 500,
    });
    const projectedCost = score.apiEquivalentCost;
    if (!projectedCost) {
      throw new Error("priced rows must keep the complete cost object");
    }
    const projectedMicros = score.apiEquivalentCostMicros;
    if (projectedMicros === undefined) {
      throw new Error("priced rows must keep cost micros");
    }
    expect(
      rankRows([
        {
          apiEquivalentCost: projectedCost,
          apiEquivalentCostMicros: projectedMicros,
          displayName: "Modeled",
          tokenScore: score.tokenScore,
          touchGrassId: "TG-MODELED",
        },
      ]),
    ).toEqual([
      {
        apiEquivalentCost: score.apiEquivalentCost,
        apiEquivalentCostMicros: 3_000_000,
        displayName: "Modeled",
        rank: 1,
        tokenScore: 500,
        touchGrassId: "TG-MODELED",
      },
    ]);
  });

  test("legacy cost micros survive without invented metadata", () => {
    expect(
      calculateScore(
        [
          {
            apiEquivalentCostMicros: 1_000_000,
            costIsComplete: true,
            observedTokens: 100,
            provider: "codex",
            rankingDay: "2026-08-05",
          },
          {
            apiEquivalentCost: {
              coveragePercent: 50,
              micros: 2_000_000,
              pricingBasis: "anthropic-standard-2026-08-07-v1",
              quality: "modeled",
            },
            apiEquivalentCostMicros: 2_000_000,
            costIsComplete: false,
            observedTokens: 300,
            provider: "claude",
            rankingDay: "2026-08-06",
          },
        ],
        "combined",
        7,
        "2026-08-06",
      ),
    ).toEqual({
      apiEquivalentCost: undefined,
      apiEquivalentCostMicros: 3_000_000,
      tokenScore: 400,
    });
  });
});
