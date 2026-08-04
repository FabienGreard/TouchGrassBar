import { expect, test } from "@playwright/test";

const fixtures = [
  "loading",
  "unavailable",
  "current",
  "update",
  "stale",
] as const;

for (const fixture of fixtures) {
  test(`${fixture} panel matches the approved presentation`, async ({
    page,
  }) => {
    await page.emulateMedia({ colorScheme: "light", reducedMotion: "reduce" });
    await page.goto(`/?fixture=${fixture}`, { waitUntil: "networkidle" });
    await page.evaluate(() => document.fonts.ready);

    const panel = page.locator('[data-slot="panel-shell"]');
    await expect(panel).toBeVisible();
    await expect(panel).toHaveScreenshot(`${fixture}-panel.png`);
  });
}

test("development preview exposes every native surface", async ({ page }) => {
  await page.goto("/?fixture=current", { waitUntil: "networkidle" });

  const previewSwitcher = page.locator('[data-slot="dev-preview-switcher"]');
  await expect(previewSwitcher).toBeVisible();
  await previewSwitcher
    .getByRole("button", { name: "Expand development preview" })
    .click();
  await expect(
    previewSwitcher.getByRole("link", { name: "Panel" }),
  ).toHaveAttribute("aria-current", "page");
  await expect(
    previewSwitcher.getByRole("link", { name: "Settings" }),
  ).toHaveAttribute("href", "?window=settings&fixture=current");
  await expect(
    previewSwitcher.getByRole("link", { name: "Onboarding" }),
  ).toHaveAttribute("href", "?window=onboarding&fixture=current");
});

test("Add Tokenmaxxer validation stays in the desktop workflow", async ({
  page,
}) => {
  await page.goto("/?fixture=current", { waitUntil: "networkidle" });

  const openDialog = async () => {
    await page.getByRole("button", { name: "Open panel menu" }).click();
    await page.getByRole("menuitem", { name: "Add a Tokenmaxxer" }).click();
  };

  await openDialog();
  const dialog = page.getByRole("dialog", { name: "Add a Tokenmaxxer" });
  const touchGrassId = dialog.getByLabel("TouchGrass ID");
  const submit = dialog.getByRole("button", {
    exact: true,
    name: "Add Tokenmaxxer",
  });

  await expect(dialog).toBeVisible();
  await expect(submit).toBeDisabled();
  await touchGrassId.fill("not-an-id");
  await expect(dialog.getByText("Use the format TG-ABC123.")).toBeVisible();
  await touchGrassId.fill("#tg-abc123");
  await expect(submit).toBeEnabled();
  await submit.click();
  await expect(
    dialog.getByText("Tokenmaxxer lookup is not connected yet."),
  ).toBeVisible();
  await dialog.getByRole("button", { name: "Cancel" }).click();

  await openDialog();
  await expect(dialog.getByLabel("TouchGrass ID")).toHaveValue("");
  await expect(
    dialog.getByRole("button", { exact: true, name: "Add Tokenmaxxer" }),
  ).toBeDisabled();
});

test("copying the current profile shows temporary text feedback", async ({
  context,
  page,
}) => {
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  await page.goto("/?fixture=current", { waitUntil: "networkidle" });

  await page
    .getByRole("button", {
      name: "Copy current user profile Fabien#TG-7K4P9D",
    })
    .click();

  await expect(page.getByText("Copied", { exact: true })).toBeVisible();
  await expect
    .poll(() => page.evaluate(() => navigator.clipboard.readText()))
    .toBe("Fabien#TG-7K4P9D");
  await expect(page.getByText("Copied", { exact: true })).toBeHidden({
    timeout: 2_500,
  });
});

test("menu hover stays quieter than the active option", async ({ page }) => {
  await page.goto("/?fixture=current", { waitUntil: "networkidle" });
  await page.getByRole("button", { name: "Select Doomerboard period" }).click();

  const active = page.getByRole("menuitemradio", { name: "Today" });
  const hovered = page.getByRole("menuitemradio", { name: "7 days" });
  await hovered.hover();

  await expect(
    active.locator('[data-slot="dropdown-menu-radio-item-indicator"]'),
  ).toBeHidden();
  await expect(page.locator('[data-slot="dropdown-menu-content"]')).toHaveCSS(
    "width",
    "92px",
  );

  const styles = await Promise.all([
    active.evaluate((element) => getComputedStyle(element).backgroundImage),
    hovered.evaluate((element) => ({
      color: getComputedStyle(element).backgroundColor,
      image: getComputedStyle(element).backgroundImage,
    })),
  ]);

  expect(styles[0]).not.toBe("none");
  expect(styles[1].image).toBe("none");
  expect(styles[1].color).not.toBe("rgba(0, 0, 0, 0)");
});

test("development preview links never use the browser focus outline", async ({
  page,
}) => {
  await page.goto("/?fixture=current", { waitUntil: "networkidle" });
  await page
    .getByRole("button", { name: "Expand development preview" })
    .click();

  const currentFixture = page.getByRole("link", {
    name: "Current",
    exact: true,
  });
  await currentFixture.focus();
  await expect
    .poll(() =>
      currentFixture.evaluate(
        (element) => getComputedStyle(element).outlineStyle,
      ),
    )
    .toBe("none");
  await expect
    .poll(() =>
      currentFixture.evaluate((element) => getComputedStyle(element).boxShadow),
    )
    .not.toBe("none");
});

test("Doomerboard rankings use the shared non-selectable scroll area", async ({
  page,
}) => {
  await page.goto("/?fixture=current", { waitUntil: "networkidle" });

  const scrollArea = page.locator('[data-slot="scroll-area-root"]');
  const viewport = page.locator('[data-slot="scroll-area-viewport"]');

  await expect(scrollArea).toHaveCount(1);
  await expect(viewport).toHaveCount(1);
  await expect(viewport).toHaveCSS("user-select", "none");
  await expect
    .poll(() =>
      viewport.evaluate(
        (element) => element.scrollHeight > element.clientHeight,
      ),
    )
    .toBe(true);
});

test("native panel viewport clips the shared shadow to its rounded edge", async ({
  page,
}) => {
  await page.setViewportSize({ height: 900, width: 402 });
  await page.goto("/?fixture=unavailable", { waitUntil: "networkidle" });
  await page.evaluate(() => {
    document.documentElement.dataset.nativePanel = "true";
    delete document.documentElement.dataset.desktopPreview;
    document.body.style.background = "#ff00ff";
    document.querySelector('[data-slot="dev-preview-switcher"]')?.remove();
  });

  await expect(
    page.locator('[data-slot="dev-preview-switcher"]'),
  ).toHaveCount(0);
  const root = page.locator("#root");
  await expect(root).toHaveCSS("border-radius", "17px");
  await expect(root).toHaveCSS("overflow", "hidden");

  const panelHeight = await page
    .locator('[data-slot="panel-shell"]')
    .evaluate((panel) => Math.ceil(panel.getBoundingClientRect().height));
  await page.setViewportSize({ height: panelHeight, width: 402 });
  await expect(page).toHaveScreenshot("native-panel-rounded-viewport.png", {
    animations: "disabled",
  });
});
