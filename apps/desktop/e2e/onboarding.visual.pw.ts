import { expect, test, type Page } from "@playwright/test";

import {
  captureNativeWindow,
  openNativeWindowPreview,
} from "./native-window-visual";

const providerStates = [
  { key: "ready", label: "Ready" },
  { key: "needs-access", label: "Needs access" },
  { key: "not-installed", label: "Not installed" },
] as const;

type OnboardingStep = "profile" | "providers" | "recovery";
type ProviderState = (typeof providerStates)[number]["key"];

function onboardingUrl(
  step: OnboardingStep,
  codexState: ProviderState = "ready",
  claudeState: ProviderState = "not-installed",
) {
  return `/?window=onboarding&fixture=current&onboardingStep=${step}&codexState=${codexState}&providerState=${claudeState}`;
}

async function openOnboarding(
  page: Page,
  step: OnboardingStep,
  codexState?: ProviderState,
  claudeState?: ProviderState,
) {
  await openNativeWindowPreview(
    page,
    onboardingUrl(step, codexState, claudeState),
    "Onboarding steps",
  );
}

test.describe("onboarding visual states", () => {
  for (const codexState of providerStates) {
    for (const claudeState of providerStates) {
      test(`providers with Codex ${codexState.label.toLowerCase()} and Claude ${claudeState.label.toLowerCase()}`, async ({
        page,
      }) => {
        await openOnboarding(
          page,
          "providers",
          codexState.key,
          claudeState.key,
        );

        await expect(
          page.getByRole("heading", {
            level: 1,
            name: "Connect your providers",
          }),
        ).toBeVisible();
        await expect(
          page.getByText("Step 1 of 3", { exact: true }),
        ).toBeVisible();
        const cards = page.locator('[data-slot="provider-connection-card"]');
        await expect(cards).toHaveCount(2);
        await expect(
          cards
            .filter({ hasText: "Codex" })
            .locator('[data-slot="status-pill"]'),
        ).toHaveText(codexState.label);
        await expect(
          cards
            .filter({ hasText: "Claude" })
            .locator('[data-slot="status-pill"]'),
        ).toHaveText(claudeState.label);

        if (
          codexState.key === "not-installed" &&
          claudeState.key === "not-installed"
        ) {
          const viewport = page.locator('[data-slot="scroll-area-viewport"]');
          await expect
            .poll(() =>
              viewport.evaluate(
                (element) => element.scrollHeight > element.clientHeight,
              ),
            )
            .toBe(true);
          await expect(
            page.getByRole("button", { name: "Continue", exact: true }),
          ).toBeVisible();
        }

        await captureNativeWindow(
          page,
          `providers-codex-${codexState.key}-claude-${claudeState.key}-onboarding.png`,
        );
      });
    }
  }

  test("providers both not installed scrolled to the end", async ({ page }) => {
    await openOnboarding(
      page,
      "providers",
      "not-installed",
      "not-installed",
    );

    const viewport = page.locator('[data-slot="scroll-area-viewport"]');
    await viewport.evaluate((element) => {
      element.scrollTop = element.scrollHeight;
    });
    await expect
      .poll(() => viewport.evaluate((element) => element.scrollTop))
      .toBeGreaterThan(0);
    await expect(
      page
        .locator('[data-slot="provider-connection-card"]')
        .filter({ hasText: "Claude" })
        .getByRole("button", { name: "View installation steps" }),
    ).toBeVisible();
    await expect(
      page.getByRole("button", { name: "Continue", exact: true }),
    ).toBeVisible();

    await captureNativeWindow(
      page,
      "providers-both-not-installed-scrolled-onboarding.png",
    );
  });

  test("profile draft", async ({ page }) => {
    await openOnboarding(page, "profile");

    await expect(
      page.getByRole("heading", { level: 1, name: "Set up your Profile" }),
    ).toBeVisible();
    await expect(page.getByText("Step 2 of 3", { exact: true })).toBeVisible();
    await expect(
      page.locator('[data-profile-state="draft"]'),
    ).toBeVisible();
    await expect(page.getByLabel("Display name")).toHaveValue("Fabien");

    await captureNativeWindow(page, "profile-draft-onboarding.png");
  });

  test("recovery", async ({ page }) => {
    await openOnboarding(page, "recovery");

    await expect(
      page.getByRole("heading", { level: 1, name: "Finish setup" }),
    ).toBeVisible();
    await expect(page.getByText("Step 3 of 3", { exact: true })).toBeVisible();
    await expect(
      page.getByText("Local setup ready", { exact: true }),
    ).toBeVisible();
    await expect(
      page.getByText(
        "Profile creation and recovery are not connected in this build.",
        { exact: true },
      ),
    ).toBeVisible();
    await expect(page.getByLabel("Recovery Key")).toHaveCount(0);
    await expect(page.locator('input[type="password"]')).toHaveCount(0);
    await expect(page.getByRole("button", { name: "Restore it" })).toHaveCount(
      0,
    );
    await expect(page.getByRole("button", { name: "Reveal key" })).toHaveCount(
      0,
    );
    await expect(
      page.getByRole("button", { name: "Finish setup", exact: true }),
    ).toBeVisible();

    await captureNativeWindow(page, "recovery-onboarding.png");
  });

});

test("onboarding steps use the shared navigation step variant", async ({
  page,
}) => {
  await openOnboarding(page, "providers");

  const navigation = page.getByRole("navigation", {
    name: "Onboarding steps",
  });
  const items = navigation.locator('[data-slot="native-window-nav-item"]');
  await expect(items).toHaveCount(3);
  for (let index = 0; index < 3; index += 1) {
    await expect(items.nth(index)).toHaveAttribute("data-variant", "step");
  }
  await expect(items.first()).toHaveAttribute("aria-current", "step");
  await expect(items.first()).toHaveCSS("font-size", "11px");
  await expect(items.nth(1)).toHaveCSS("font-size", "11px");
  await expect(
    page.locator('[data-slot="provider-connection-card"]'),
  ).toHaveCount(2);
  await expect(page.locator('[data-slot="scroll-area-root"]')).toHaveCount(1);
  await expect(
    page.getByRole("link", { name: /onboarding variant/i }),
  ).toHaveCount(0);
});
