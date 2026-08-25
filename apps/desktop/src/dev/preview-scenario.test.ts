import { describe, expect, test } from "vitest";

import {
  resolveDevPreviewScenario,
  syncPreviewStatuses,
  updatePreviewStatuses,
} from "@/dev/preview-scenario";

describe("development preview scenarios", () => {
  test("resolves explicit panel fixtures and rejects unknown values", () => {
    expect(resolveDevPreviewScenario("?fixture=current").fixture).toBe("current");
    expect(resolveDevPreviewScenario("?fixture=current").updateStatus).toBe("idle");
    expect(resolveDevPreviewScenario("?fixture=update")).toMatchObject({
      fixture: "update",
      updateStatus: "available",
    });
    expect(resolveDevPreviewScenario("?fixture=anything-else").fixture).toBe("unavailable");
  });

  test.each(updatePreviewStatuses)("resolves the $key update preview", ({ key }) => {
    expect(
      resolveDevPreviewScenario(`?fixture=current&updateStatus=${encodeURIComponent(key)}`),
    ).toMatchObject({ fixture: "current", updateStatus: key });
  });

  test("rejects unknown update preview details", () => {
    const scenario = resolveDevPreviewScenario(
      "?fixture=current&updateStatus=restarting&updateDetail=private-detail",
    );

    expect(scenario.updateStatus).toBe("idle");
    expect(scenario).not.toHaveProperty("updateDetail");
  });

  test.each(syncPreviewStatuses)(
    "resolves the $key sync status independently from provider freshness",
    ({ key }) => {
      expect(
        resolveDevPreviewScenario(`?fixture=stale&syncStatus=${encodeURIComponent(key)}`),
      ).toMatchObject({ fixture: "stale", syncStatus: key });
    },
  );

  test("rejects unknown sync status details", () => {
    const scenario = resolveDevPreviewScenario(
      "?fixture=current&syncStatus=retrying&syncReason=private-detail",
    );

    expect(scenario).toMatchObject({
      fixture: "current",
      syncStatus: "unavailable",
    });
    expect(scenario).not.toHaveProperty("syncReason");
  });

  test("owns surface and onboarding query parsing outside production", () => {
    expect(
      resolveDevPreviewScenario(
        "?window=onboarding&onboardingStep=profile&codexState=needs-access&providerState=detected&setupState=profile-pending",
      ),
    ).toMatchObject({
      onboarding: {
        codexState: "needs-access",
        initialStep: "profile",
        providerState: "detected",
        setupState: "profile-pending",
      },
      settingsProfileState: "saved",
      settingsProviderEnabled: true,
      settingsProviderState: "detected",
      surface: "onboarding",
    });
  });

  test("resolves the Settings-only excluded provider state", () => {
    expect(resolveDevPreviewScenario("?window=settings&providerState=excluded")).toMatchObject({
      settingsProviderEnabled: false,
      settingsProviderState: "not-installed",
      surface: "settings",
    });
  });

  test("resolves the Profile Pending Settings fixture", () => {
    expect(
      resolveDevPreviewScenario("?window=settings&profileState=profile-pending")
        .settingsProfileState,
    ).toBe("profile-pending");
  });
});
