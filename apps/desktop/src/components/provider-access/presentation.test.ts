import { describe, expect, test } from "vitest";

import {
  providerAccessPresentations,
  settingsProviderAccessPresentations,
} from "@/components/provider-access/presentation";

describe("provider access presentation", () => {
  test("preserves the Settings enabled value", () => {
    expect(
      settingsProviderAccessPresentations([
        {
          displayName: "Claude",
          enabled: false,
          provider: "claude",
          status: "detected",
        },
      ]),
    ).toEqual([
      {
        displayName: "Claude",
        enabled: false,
        provider: "claude",
        state: "detected",
      },
    ]);
  });

  test("keeps the Bootstrap presentation independent of Settings", () => {
    expect(
      providerAccessPresentations([
        {
          displayName: "Codex",
          provider: "codex",
          status: "not-detected",
        },
      ]),
    ).toEqual([
      {
        displayName: "Codex",
        provider: "codex",
        state: "not-installed",
      },
    ]);
  });
});
