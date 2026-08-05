import { describe, expect, test } from "vitest";

import {
  bootstrapStateSchema,
  refreshReceiptSchema,
  sanitizedDesktopStateSchema,
  settingsStateSchema,
  tokenmaxxerSchema,
} from "./index";

const unavailableState = {
  contractVersion: 2,
  generatedAt: "2026-08-03T00:00:00.000Z",
  profile: { status: "not-authorized" },
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

  test("accepts only the Rust-owned refresh acknowledgement shape", () => {
    expect(refreshReceiptSchema.parse({ accepted: true })).toEqual({
      accepted: true,
    });
    expect(refreshReceiptSchema.safeParse({ accepted: "yes" }).success).toBe(
      false,
    );
    expect(
      refreshReceiptSchema.safeParse({ accepted: true, revision: "2" }).success,
    ).toBe(false);
  });

  test("strictly validates the Rust-owned bootstrap and Settings views", () => {
    const providers = [
      { provider: "codex", status: "detected" },
      { provider: "claude", status: "not-detected" },
    ] as const;
    const bootstrap = {
      bootstrap: "completed",
      contractVersion: 2,
      displayName: "Fabien",
      persistence: "available",
      profileProvisioning: "profile-pending",
      providers,
    } as const;
    const settings = {
      contractVersion: 2,
      displayName: "Fabien",
      launchAtLogin: { availability: "available", enabled: true },
      profileProvisioning: "profile-pending",
      providers,
      section: "profile",
    } as const;

    expect(bootstrapStateSchema.parse(bootstrap)).toEqual(bootstrap);
    expect(settingsStateSchema.parse(settings)).toEqual(settings);
    expect(
      bootstrapStateSchema.safeParse({ ...bootstrap, localPath: "/private" })
        .success,
    ).toBe(false);
    expect(
      settingsStateSchema.safeParse({ ...settings, contractVersion: 3 })
        .success,
    ).toBe(false);
  });

  test.each([
    [
      "an unknown contract version",
      { ...unavailableState, contractVersion: 3 },
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
