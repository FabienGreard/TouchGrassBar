import { describe, expect, test } from "vitest";

import { createBrowserSanitizedDesktopStateAdapter } from "@/dev/browser-sanitized-desktop-state-adapter";
import { createSanitizedDesktopStateDelivery } from "@/native-state/sanitized-desktop-state-delivery";

const fixedNow = () => new Date("2026-08-03T00:00:00.000Z");

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
        fixedNow,
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

  test("keeps an excluded provider visible with unavailable panel data", async () => {
    const delivery = createSanitizedDesktopStateDelivery(
      createBrowserSanitizedDesktopStateAdapter(
        "current",
        fixedNow,
        { claude: false, codex: true },
      ),
    );
    const unsubscribe = delivery.subscribe(() => undefined);

    await waitForRevision(delivery, "2");

    const snapshot = delivery.getSnapshot().snapshot;
    expect(snapshot?.providers).toHaveLength(2);
    expect(snapshot?.providers[1]).toMatchObject({
      provider: "claude",
      quota: { availability: "unavailable", quotaLanes: [] },
      usage: {
        thirtyDays: { availability: "unavailable" },
        today: { availability: "unavailable" },
      },
    });
    expect(snapshot?.combinedUsage.today).toMatchObject({
      availability: "current",
      observedTokens: 12_800_000,
    });
    unsubscribe();
  });

  test("recomputes Combined from the enabled preview providers", async () => {
    const bothDelivery = createSanitizedDesktopStateDelivery(
      createBrowserSanitizedDesktopStateAdapter("current", fixedNow, {
        claude: true,
        codex: true,
      }),
    );
    const claudeDelivery = createSanitizedDesktopStateDelivery(
      createBrowserSanitizedDesktopStateAdapter("current", fixedNow, {
        claude: true,
        codex: false,
      }),
    );
    const stopBoth = bothDelivery.subscribe(() => undefined);
    const stopClaude = claudeDelivery.subscribe(() => undefined);

    await waitForRevision(bothDelivery, "2");
    await waitForRevision(claudeDelivery, "2");

    const both = bothDelivery.getSnapshot().snapshot;
    const claudeOnly = claudeDelivery.getSnapshot().snapshot;
    const codexToday = both?.providers[0]?.usage.today;
    const claudeToday = both?.providers[1]?.usage.today;
    expect(codexToday?.availability).toBe("current");
    expect(claudeToday?.availability).toBe("current");
    if (
      both?.combinedUsage.today.availability !== "current" ||
      codexToday?.availability !== "current" ||
      claudeToday?.availability !== "current"
    ) {
      throw new Error("Current preview usage is required");
    }
    expect(both.combinedUsage.today.observedTokens).toBe(
      codexToday.observedTokens + claudeToday.observedTokens,
    );
    expect(claudeOnly?.providers[0]).toMatchObject({
      provider: "codex",
      quota: { availability: "unavailable", quotaLanes: [] },
      usage: { today: { availability: "unavailable" } },
    });
    expect(claudeOnly?.combinedUsage.today).toEqual(
      claudeOnly?.providers[1]?.usage.today,
    );
    stopBoth();
    stopClaude();
  });
});
