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

type SettingsSection = "general" | "profile" | "providers";

function settingsUrl(
  section: SettingsSection,
  providerState: (typeof providerStates)[number]["key"] = "not-installed",
) {
  return `/?window=settings&fixture=current&providerState=${providerState}#settings-${section}`;
}

async function openSettings(
  page: Page,
  section: SettingsSection,
  providerState?: (typeof providerStates)[number]["key"],
) {
  await openNativeWindowPreview(
    page,
    settingsUrl(section, providerState),
    "Settings sections",
  );
  await expect(
    page.getByRole("heading", {
      level: 1,
      name: section[0]?.toUpperCase() + section.slice(1),
    }),
  ).toBeVisible();
}

test.describe("settings visual states", () => {
  test("general defaults", async ({ page }) => {
    await openSettings(page, "general");

    await expect(
      page.getByText("Start quietly in the menu bar.", { exact: true }),
    ).toBeVisible();
    await expect(page.getByRole("switch", { name: "Open at login" })).not.toBeChecked();
    await expect(
      page.getByRole("switch", { name: "Check automatically" }),
    ).toBeChecked();

    await captureNativeWindow(page, "general-default-settings.png");
  });

  test("general customized toggles", async ({ page }) => {
    await openSettings(page, "general");

    const launchAtLogin = page.getByRole("switch", { name: "Open at login" });
    const automaticUpdates = page.getByRole("switch", {
      name: "Check automatically",
    });
    await expect(launchAtLogin).toBeEnabled();
    await launchAtLogin.click();
    await automaticUpdates.click();
    await expect(launchAtLogin).toBeChecked();
    await expect(automaticUpdates).not.toBeChecked();

    await captureNativeWindow(page, "general-customized-settings.png");
  });

  for (const providerState of providerStates) {
    test(`providers with Claude ${providerState.label.toLowerCase()}`, async ({
      page,
    }) => {
      await openSettings(page, "providers", providerState.key);

      const cards = page.locator('[data-slot="provider-connection-card"]');
      const codexCard = cards.filter({ hasText: "Codex" });
      const claudeCard = cards.filter({ hasText: "Claude" });
      await expect(cards).toHaveCount(2);
      await expect(codexCard.locator('[data-slot="status-pill"]')).toHaveText(
        "Ready",
      );
      await expect(claudeCard.locator('[data-slot="status-pill"]')).toHaveText(
        providerState.label,
      );

      await captureNativeWindow(
        page,
        `providers-claude-${providerState.key}-settings.png`,
      );
    });
  }

  test("profile saved", async ({ page }) => {
    await openSettings(page, "profile");

    await expect(page.locator('[data-slot="profile-settings"]')).toBeVisible();
    await expect(
      page.locator('[data-profile-state="saved"]'),
    ).toBeVisible();
    await expect(
      page.getByText("Profile security", { exact: true }),
    ).toBeVisible();
    await expect(
      page.getByText(
        "Recovery and key access require a secure macOS sheet and are not available in this build.",
        { exact: true },
      ),
    ).toBeVisible();
    await expect(page.getByLabel("Recovery Key")).toHaveCount(0);
    await expect(page.locator('input[type="password"]')).toHaveCount(0);
    await expect(
      page.getByRole("button", { name: /Reveal recovery key/ }),
    ).toHaveCount(0);
    await expect(
      page.getByRole("button", { name: /Enter Recovery Key/ }),
    ).toHaveCount(0);
    await expect(page.getByText(/TG-RK-/)).toHaveCount(0);

    await captureNativeWindow(page, "profile-saved-settings.png");
  });

  test("profile display name editing", async ({ page }) => {
    await openSettings(page, "profile");

    await page.getByRole("button", { name: "Edit", exact: true }).click();
    await expect(
      page.locator('[data-profile-state="editing"]'),
    ).toBeVisible();
    await expect(page.getByLabel("Display name")).toHaveValue("Fabien");

    await captureNativeWindow(page, "profile-editing-settings.png");
  });

});

test("settings controls never use the browser focus outline", async ({
  page,
}) => {
  await openSettings(page, "providers", "not-installed");

  const providers = page.getByRole("button", {
    name: "Providers",
    exact: true,
  });
  await providers.focus();
  await expect
    .poll(() =>
      providers.evaluate((element) => getComputedStyle(element).outlineStyle),
    )
    .toBe("none");
  await expect
    .poll(() =>
      providers.evaluate((element) => getComputedStyle(element).boxShadow),
    )
    .not.toBe("none");

  const installationSteps = page.getByRole("button", {
    name: "View installation steps",
    exact: true,
  });
  await installationSteps.focus();
  await expect(installationSteps).toHaveCSS("outline-style", "none");
  await expect(installationSteps).toHaveCSS(
    "text-decoration-line",
    "underline",
  );
  await expect(installationSteps).toHaveCSS(
    "text-decoration-thickness",
    "1px",
  );
  await expect(installationSteps).toHaveCSS(
    "text-decoration-color",
    "rgb(139, 234, 75)",
  );
});

test("settings navigation shares the panel menu selection model", async ({
  page,
}) => {
  await openSettings(page, "general");

  const general = page.getByRole("button", { name: "General", exact: true });
  const providers = page.getByRole("button", {
    name: "Providers",
    exact: true,
  });
  await expect(general).toHaveAttribute("aria-current", "page");
  await providers.hover();
  await expect
    .poll(() =>
      providers.evaluate(
        (element) => getComputedStyle(element).backgroundColor,
      ),
    )
    .not.toBe("rgba(0, 0, 0, 0)");

  await providers.click();
  await expect(providers).toHaveAttribute("aria-current", "page");
  await expect(general).not.toHaveAttribute("aria-current", "page");
  await expect
    .poll(() =>
      providers.evaluate(
        (element) => getComputedStyle(element).backgroundImage,
      ),
    )
    .not.toBe("none");
});

test("native settings keeps scrolling inside the content pane", async ({
  page,
}) => {
  await openSettings(page, "providers", "not-installed");
  await page.setViewportSize({ height: 552, width: 760 });
  await page.evaluate(() => {
    delete document.documentElement.dataset.desktopPreview;
    document.documentElement.dataset.nativeRuntime = "true";
    document
      .querySelectorAll(
        '[data-slot="dev-preview-switcher"], [data-slot="dev-preview-restore"]',
      )
      .forEach((element) => element.remove());
  });

  const layout = await page.evaluate(() => {
    const sidebar = document.querySelector<HTMLElement>(
      '[data-slot="native-window-sidebar"]',
    );
    const content = document.querySelector<HTMLElement>(
      '[data-slot="native-window-content"]',
    );

    if (!sidebar || !content) throw new Error("Settings layout is missing");

    return {
      contentOverflowY: getComputedStyle(content).overflowY,
      documentClientHeight: document.documentElement.clientHeight,
      documentScrollHeight: document.documentElement.scrollHeight,
      sidebarBottom: Math.round(sidebar.getBoundingClientRect().bottom),
      sidebarClientHeight: sidebar.clientHeight,
      viewportHeight: window.innerHeight,
    };
  });

  expect(layout.documentScrollHeight).toBe(layout.documentClientHeight);
  expect(layout.sidebarBottom).toBe(layout.viewportHeight);
  expect(layout.sidebarClientHeight).toBe(layout.viewportHeight);
  expect(layout.contentOverflowY).toBe("auto");

  await page.setViewportSize({ height: 420, width: 760 });
  const constrainedLayout = await page.evaluate(() => {
    const sidebar = document.querySelector<HTMLElement>(
      '[data-slot="native-window-sidebar"]',
    );
    const content = document.querySelector<HTMLElement>(
      '[data-slot="native-window-content"]',
    );

    if (!sidebar || !content) throw new Error("Settings layout is missing");

    return {
      contentClientHeight: content.clientHeight,
      contentScrollHeight: content.scrollHeight,
      documentClientHeight: document.documentElement.clientHeight,
      documentScrollHeight: document.documentElement.scrollHeight,
      sidebarBottom: Math.round(sidebar.getBoundingClientRect().bottom),
      sidebarTop: Math.round(sidebar.getBoundingClientRect().top),
    };
  });

  expect(constrainedLayout.documentScrollHeight).toBe(
    constrainedLayout.documentClientHeight,
  );
  expect(constrainedLayout.contentScrollHeight).toBeGreaterThan(
    constrainedLayout.contentClientHeight,
  );

  const afterContentScroll = await page.evaluate(() => {
    const sidebar = document.querySelector<HTMLElement>(
      '[data-slot="native-window-sidebar"]',
    );
    const content = document.querySelector<HTMLElement>(
      '[data-slot="native-window-content"]',
    );

    if (!sidebar || !content) throw new Error("Settings layout is missing");
    content.scrollTo({ top: content.scrollHeight });

    return {
      contentScrollTop: content.scrollTop,
      documentScrollTop: document.documentElement.scrollTop,
      sidebarBottom: Math.round(sidebar.getBoundingClientRect().bottom),
      sidebarTop: Math.round(sidebar.getBoundingClientRect().top),
    };
  });

  expect(afterContentScroll.contentScrollTop).toBeGreaterThan(0);
  expect(afterContentScroll.documentScrollTop).toBe(0);
  expect(afterContentScroll.sidebarTop).toBe(constrainedLayout.sidebarTop);
  expect(afterContentScroll.sidebarBottom).toBe(
    constrainedLayout.sidebarBottom,
  );
});
