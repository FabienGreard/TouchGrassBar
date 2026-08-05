import { describe, expect, test } from "vitest";

import { resolveDevPreviewScenario } from "@/dev/preview-scenario";

describe("development preview scenarios", () => {
  test("resolves explicit panel fixtures and rejects unknown values", () => {
    expect(resolveDevPreviewScenario("?fixture=current").fixture).toBe(
      "current",
    );
    expect(resolveDevPreviewScenario("?fixture=update").fixture).toBe("update");
    expect(resolveDevPreviewScenario("?fixture=anything-else").fixture).toBe(
      "unavailable",
    );
  });

  test("owns surface and onboarding query parsing outside production", () => {
    expect(
      resolveDevPreviewScenario(
        "?window=onboarding&onboardingStep=profile&codexState=needs-access&providerState=ready&setupState=profile-pending",
      ),
    ).toMatchObject({
      onboarding: {
        codexState: "needs-access",
        initialStep: "profile",
        providerState: "ready",
        setupState: "profile-pending",
      },
      settingsProfileState: "saved",
      settingsProviderState: "ready",
      surface: "onboarding",
    });
  });

  test("resolves the Profile Pending Settings fixture", () => {
    expect(
      resolveDevPreviewScenario(
        "?window=settings&profileState=profile-pending",
      ).settingsProfileState,
    ).toBe("profile-pending");
  });
});
