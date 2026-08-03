import { describe, expect, test } from "vitest";

import { sanitizedDesktopStateSchema, tokenmaxxerSchema } from "./index";

describe("public contracts", () => {
  test("accepts a canonical TouchGrass ID", () => {
    expect(
      tokenmaxxerSchema.parse({
        displayName: "Fabien",
        touchGrassId: "TG-7K4P9D",
      }),
    ).toEqual({ displayName: "Fabien", touchGrassId: "TG-7K4P9D" });
  });

  test("rejects raw provider material at the IPC boundary", () => {
    const result = sanitizedDesktopStateSchema.safeParse({
      contractVersion: 1,
      generatedAt: "2026-08-03T00:00:00.000Z",
      providers: [],
      rawLog: "must never reach React",
      sync: { lastSuccessfulAt: null, status: "unavailable" },
      usage: {
        claude: {
          thirtyDays: { apiEquivalentCostUsd: null, costIsComplete: false, observedTokens: 0 },
          sevenDays: { apiEquivalentCostUsd: null, costIsComplete: false, observedTokens: 0 },
          today: { apiEquivalentCostUsd: null, costIsComplete: false, observedTokens: 0 },
        },
        codex: {
          thirtyDays: { apiEquivalentCostUsd: null, costIsComplete: false, observedTokens: 0 },
          sevenDays: { apiEquivalentCostUsd: null, costIsComplete: false, observedTokens: 0 },
          today: { apiEquivalentCostUsd: null, costIsComplete: false, observedTokens: 0 },
        },
      },
    });

    expect(result.success).toBe(false);
  });
});
