import type { SanitizedDesktopState } from "@touchgrass/contracts";
import { sanitizedDesktopStateSchema } from "@touchgrass/contracts";
import { describe, expect, test } from "vitest";

import {
  acceptNewerSnapshot,
  browserFixture,
  resolveBrowserFixtureName,
  shouldHidePanel,
  unavailableBrowserFixture,
} from "./nativeState";

function state(revision: string): SanitizedDesktopState {
  return {
    contractVersion: 1,
    generatedAt: "2026-08-03T00:00:00.000Z",
    revision,
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
  };
}

describe("native state ordering", () => {
  test("keeps the development browser fixture behind the generated validator", () => {
    expect(
      sanitizedDesktopStateSchema.parse(
        unavailableBrowserFixture(new Date("2026-08-03T00:00:00.000Z")),
      ).usage.codex.today,
    ).toEqual({ availability: "unavailable" });
  });

  test("offers validated unavailable, current, update, and stale development fixtures", () => {
    const now = new Date("2026-08-03T00:00:00.000Z");
    const unavailable = sanitizedDesktopStateSchema.parse(
      browserFixture("unavailable", now),
    );
    const current = sanitizedDesktopStateSchema.parse(
      browserFixture("current", now),
    );
    const update = sanitizedDesktopStateSchema.parse(
      browserFixture("update", now),
    );
    const stale = sanitizedDesktopStateSchema.parse(
      browserFixture("stale", now),
    );

    expect(unavailable.usage.codex.today).toEqual({
      availability: "unavailable",
    });
    expect(current.providers[0]).toMatchObject({
      availability: "current",
      provider: "codex",
    });
    expect(current.providers[0]?.quotaLanes.map((lane) => lane.label)).toEqual([
      "Weekly limit",
      "5-hour limit",
    ]);
    expect(current.providers[1]?.quotaLanes.map((lane) => lane.label)).toEqual([
      "Weekly limit",
      "5-hour limit",
    ]);
    expect(current.usage.codex.today).toMatchObject({
      availability: "current",
      observedTokens: 12_800_000,
    });
    expect(update.sync.status).toBe("synced");
    expect(stale.sync.status).toBe("stale");
    expect(
      stale.providers.every((provider) => provider.availability === "stale"),
    ).toBe(true);
  });

  test("selects fixtures only from an explicit development query", () => {
    expect(resolveBrowserFixtureName("?fixture=loading")).toBe("loading");
    expect(resolveBrowserFixtureName("?fixture=current")).toBe("current");
    expect(resolveBrowserFixtureName("?fixture=update")).toBe("update");
    expect(resolveBrowserFixtureName("?fixture=stale")).toBe("stale");
    expect(resolveBrowserFixtureName("?fixture=anything-else")).toBe(
      "unavailable",
    );
    expect(resolveBrowserFixtureName("")).toBe("unavailable");
  });

  test("replaces state only with a higher revision", () => {
    const current = state("9007199254740993");
    expect(acceptNewerSnapshot(current, state("9007199254740992"))).toBe(
      current,
    );
    expect(acceptNewerSnapshot(current, state("9007199254740993"))).toBe(
      current,
    );
    expect(
      acceptNewerSnapshot(current, state("9007199254740994")).revision,
    ).toBe("9007199254740994");
  });

  test("supports native panel dismissal shortcuts", () => {
    expect(shouldHidePanel({ key: "Escape", metaKey: false })).toBe(true);
    expect(shouldHidePanel({ key: "w", metaKey: true })).toBe(true);
    expect(shouldHidePanel({ key: "w", metaKey: false })).toBe(false);
  });
});
