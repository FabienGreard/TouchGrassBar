import { describe, expect, test } from "vitest";

import { rankRows } from "./doomerboards";
import { calculateScore } from "./model/scores";

describe("cost and ranking independence", () => {
  test("a cost-only reprice does not change Token Score or rank", () => {
    const baseRow = {
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
      [{ ...baseRow, apiEquivalentCostMicros: 250_000 }],
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
});
