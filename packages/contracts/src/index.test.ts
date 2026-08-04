import { describe, expect, test } from "vitest";

import { sanitizedDesktopStateSchema, tokenmaxxerSchema } from "./index";

const unavailableState = {
  contractVersion: 1,
  generatedAt: "2026-08-03T00:00:00.000Z",
  revision: "1",
  providers: [
    { availability: "unavailable", provider: "codex", quotaLanes: [] },
    { availability: "unavailable", provider: "claude", quotaLanes: [] },
  ],
  sync: { lastSuccessfulAt: null, status: "unavailable" },
  usage: {
    claude: {
      thirtyDays: { availability: "unavailable" },
      sevenDays: { availability: "unavailable" },
      today: { availability: "unavailable" },
    },
    codex: {
      thirtyDays: { availability: "unavailable" },
      sevenDays: { availability: "unavailable" },
      today: { availability: "unavailable" },
    },
  },
} as const;

describe("public contracts", () => {
  test("accepts a canonical TouchGrass ID", () => {
    expect(
      tokenmaxxerSchema.parse({
        displayName: "Fabien",
        touchGrassId: "TG-7K4P9D",
      }),
    ).toEqual({ displayName: "Fabien", touchGrassId: "TG-7K4P9D" });
  });

  test("accepts an honestly unavailable native snapshot without invented zeroes", () => {
    expect(sanitizedDesktopStateSchema.parse(unavailableState)).toEqual(
      unavailableState,
    );
    expect(JSON.stringify(unavailableState)).not.toContain("observedTokens");
  });

  test.each([
    [
      "an unknown contract version",
      { ...unavailableState, contractVersion: 2 },
    ],
    [
      "raw provider material",
      { ...unavailableState, rawLog: "must never reach React" },
    ],
    ["session material", { ...unavailableState, sessionToken: "secret" }],
    ["an unsafe numeric revision", { ...unavailableState, revision: 2 }],
  ])("rejects %s", (_name, payload) => {
    expect(sanitizedDesktopStateSchema.safeParse(payload).success).toBe(false);
  });
});
