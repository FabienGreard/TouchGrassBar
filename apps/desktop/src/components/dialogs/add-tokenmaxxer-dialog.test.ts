import { describe, expect, test } from "vitest";

import { normalizeTouchGrassId } from "./add-tokenmaxxer";

describe("Add Tokenmaxxer dialog validation", () => {
  test.each([
    ["TG-ABC123", "TG-ABC123"],
    ["  tg-abc123  ", "TG-ABC123"],
    ["#TG-ABC123", "TG-ABC123"],
  ])("normalizes %s", (value, expected) => {
    expect(normalizeTouchGrassId(value)).toBe(expected);
  });
});
