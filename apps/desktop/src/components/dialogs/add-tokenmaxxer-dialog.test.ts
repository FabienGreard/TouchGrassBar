import { describe, expect, test } from "vitest";

import {
  addTokenmaxxerHelpText,
  normalizeTouchGrassId,
  validTouchGrassId,
} from "./add-tokenmaxxer";

describe("Add Tokenmaxxer dialog validation", () => {
  test.each([
    ["TG-ABC123", "TG-ABC123"],
    ["  tg-abc123  ", "TG-ABC123"],
    ["#TG-ABC123", "TG-ABC123"],
  ])("normalizes %s", (value, expected) => {
    expect(normalizeTouchGrassId(value)).toBe(expected);
  });

  test("accepts only the canonical public TouchGrass ID alphabet", () => {
    expect(validTouchGrassId("TG-ABC234")).toBe(true);
    expect(validTouchGrassId("TG-ABC123")).toBe(false);
    expect(validTouchGrassId("TG-ABCI23")).toBe(false);
    expect(validTouchGrassId("TG-ABCO23")).toBe(false);
  });

  test.each([
    ["idle", "Ask the Tokenmaxxer for their TouchGrass ID."],
    ["submitting", "Adding Tokenmaxxer…"],
    ["already-added", "Already in My Tokenmaxxers."],
    ["invalid", "Use the format TG-ABC234."],
    ["limit-reached", "My Tokenmaxxers is limited to 100."],
    ["not-found", "No Tokenmaxxer has that TouchGrass ID."],
    ["self", "That is your TouchGrass ID."],
    ["unavailable", "Adding a Tokenmaxxer is unavailable. Try again."],
  ] as const)("presents the bounded %s outcome", (status, expected) => {
    expect(addTokenmaxxerHelpText(status)).toBe(expected);
  });
});
