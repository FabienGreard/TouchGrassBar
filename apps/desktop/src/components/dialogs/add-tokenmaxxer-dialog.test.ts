import { describe, expect, test } from "vitest";

import { createAddTokenmaxxerRequestGuard, normalizeTouchGrassId } from "./add-tokenmaxxer";

describe("Add Tokenmaxxer dialog validation", () => {
  test.each([
    ["TG-ABC123", "TG-ABC123"],
    ["  tg-abc123  ", "TG-ABC123"],
    ["#TG-ABC123", "TG-ABC123"],
  ])("normalizes %s", (value, expected) => {
    expect(normalizeTouchGrassId(value)).toBe(expected);
  });

  test("blocks concurrent additions and ignores an invalidated completion", () => {
    const guard = createAddTokenmaxxerRequestGuard();
    const firstRequest = guard.begin();

    expect(firstRequest).not.toBeNull();
    expect(guard.begin()).toBeNull();
    expect(guard.inFlight()).toBe(true);

    guard.invalidate();
    expect(guard.inFlight()).toBe(true);
    expect(guard.finish(firstRequest!)).toBe(false);
    expect(guard.inFlight()).toBe(false);

    const nextRequest = guard.begin();
    expect(nextRequest).not.toBeNull();
    expect(guard.finish(nextRequest!)).toBe(true);
  });
});
