import { describe, expect, test } from "vitest";

import {
  CONTRACT_VERSION,
  bootstrapStateSchema,
  refreshReceiptSchema,
  sanitizedDesktopStateSchema,
  settingsStateSchema,
  syncStateSchema,
  tokenmaxxerSchema,
} from "./index";

const unavailableUsage = {
  scanStatus: "unavailable",
  thirtyDays: { availability: "unavailable" },
  sevenDays: { availability: "unavailable" },
  today: { availability: "unavailable" },
} as const;

const unavailableState = {
  combinedUsage: unavailableUsage,
  contractVersion: CONTRACT_VERSION,
  generatedAt: "2026-08-03T00:00:00.000Z",
  profile: { status: "not-authorized" },
  revision: "1",
  providers: [
    {
      displayName: "Codex",
      presence: "not-detected",
      provider: "codex",
      quota: { availability: "unavailable", provider: "codex", quotaLanes: [] },
      usage: unavailableUsage,
    },
    {
      displayName: "Claude",
      presence: "not-detected",
      provider: "claude",
      quota: { availability: "unavailable", provider: "claude", quotaLanes: [] },
      usage: unavailableUsage,
    },
  ],
  sync: { lastSuccessfulAt: null, status: "unavailable" },
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
    "synced",
    "pending",
    "stale",
    "offline",
    "authority-rejected",
    "unavailable",
  ] as const)("accepts the sanitized %s sync state", (status) => {
    const lastSuccessfulAt = status === "synced" || status === "stale"
      ? "2026-08-08T12:00:00.000Z"
      : null;
    expect(syncStateSchema.parse({ lastSuccessfulAt, status })).toEqual({
      lastSuccessfulAt,
      status,
    });
  });

  test.each([
    { status: "retrying" },
    { lastSuccessfulAt: "not-a-time", status: "offline" },
    { credential: "sentinel", status: "authority-rejected" },
    { installationId: "sentinel", status: "pending" },
    { status: "offline", transportError: "sentinel" },
  ])("rejects a non-sanitized sync state", (sync) => {
    expect(syncStateSchema.safeParse(sync).success).toBe(false);
  });

  test("rejects partial API-equivalent cost metadata", () => {
    const invalid = {
      ...unavailableState,
      combinedUsage: {
        ...unavailableState.combinedUsage,
        today: {
          apiEquivalentCostUsd: 1,
          availability: "current",
          coverage: "complete",
          evidenceBasis: "provider-reported",
          observedAt: "2026-08-03T00:00:00.000Z",
          observedTokens: 100,
        },
      },
    };
    expect(sanitizedDesktopStateSchema.safeParse(invalid).success).toBe(false);
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
      { displayName: "Codex", provider: "codex", status: "detected" },
      { displayName: "Claude", provider: "claude", status: "not-detected" },
    ] as const;
    const bootstrap = {
      bootstrap: "completed",
      contractVersion: 3,
      displayName: "Fabien",
      persistence: "available",
      profileProvisioning: "profile-pending",
      providers,
    } as const;
    const settings = {
      contractVersion: 4,
      displayName: "Fabien",
      launchAtLogin: { availability: "available", enabled: true },
      recoveryKeySuffix: "K9m",
      profileProvisioning: "profile-pending",
      providers: providers.map((provider) => ({
        displayName: provider.displayName,
        enabled: true,
        provider: provider.provider,
        status: provider.status,
      })),
      section: "profile",
    } as const;

    expect(bootstrapStateSchema.parse(bootstrap)).toEqual(bootstrap);
    expect(settingsStateSchema.parse(settings)).toEqual(settings);
    expect(
      bootstrapStateSchema.safeParse({ ...bootstrap, localPath: "/private" })
        .success,
    ).toBe(false);
    expect(
      settingsStateSchema.safeParse({ ...settings, contractVersion: 2 })
        .success,
    ).toBe(false);
    expect(
      settingsStateSchema.safeParse({ ...settings, recoveryKeySuffix: "0-9" })
        .success,
    ).toBe(false);
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
