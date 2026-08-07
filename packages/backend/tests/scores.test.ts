import { describe, expect, test } from "vitest";

import { rankRows } from "../convex/doomerboards";
import { calculateScore } from "../convex/model/scores";

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

    expect(after.apiEquivalentCostMicros).not.toBe(
      before.apiEquivalentCostMicros,
    );
    expect(after.tokenScore).toBe(before.tokenScore);

    const board = (apiEquivalentCostMicros: number | undefined) =>
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

    expect(board(after.apiEquivalentCostMicros)).toEqual(
      board(before.apiEquivalentCostMicros),
    );
  });
});
