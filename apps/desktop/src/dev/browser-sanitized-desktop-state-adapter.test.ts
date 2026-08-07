import { describe, expect, test } from "vitest";

import {
  createBrowserSanitizedDesktopStateAdapter,
} from "@/dev/browser-sanitized-desktop-state-adapter";
import { createSanitizedDesktopStateDelivery } from "@/native-state/sanitized-desktop-state-delivery";

async function waitForRevision(
  delivery: ReturnType<typeof createSanitizedDesktopStateDelivery>,
  revision: string,
) {
  let failure: unknown;
  for (let attempt = 0; attempt < 50; attempt += 1) {
    try {
      expect(delivery.getSnapshot().snapshot?.revision).toBe(revision);
      return;
    } catch (error) {
      failure = error;
      await Promise.resolve();
    }
  }
  throw failure;
}

describe("browser sanitized desktop state fixtures", () => {
  test("routes fixtures through the production delivery contract", async () => {
    const delivery = createSanitizedDesktopStateDelivery(
      createBrowserSanitizedDesktopStateAdapter(
        "current",
        () => new Date("2026-08-03T00:00:00.000Z"),
      ),
    );
    const unsubscribe = delivery.subscribe(() => undefined);

    await waitForRevision(delivery, "2");

    expect(delivery.getSnapshot().snapshot?.providers[0]).toMatchObject({
      displayName: "Codex",
      provider: "codex",
      quota: { availability: "current" },
    });
    expect(
      delivery
        .getSnapshot()
        .snapshot?.providers[1]?.quota.quotaLanes.map((lane) => lane.label),
    ).toEqual(["Weekly limit", "5-hour limit"]);
    unsubscribe();
  });
});
