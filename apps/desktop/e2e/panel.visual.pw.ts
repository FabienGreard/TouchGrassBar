import { expect, test } from "@playwright/test";

const fixtures = [
  "loading",
  "unavailable",
  "current",
  "update",
  "stale",
] as const;

for (const fixture of fixtures) {
  test(`${fixture} panel matches the approved presentation`, async ({ page }) => {
    await page.emulateMedia({ colorScheme: "light", reducedMotion: "reduce" });
    await page.goto(`/?fixture=${fixture}`, { waitUntil: "networkidle" });
    await page.evaluate(() => document.fonts.ready);

    const panel = page.locator('[data-slot="panel-shell"]');
    await expect(panel).toBeVisible();
    await expect(panel).toHaveScreenshot(`${fixture}-panel.png`);
  });
}

test("copying the current identity shows temporary text feedback", async ({
  context,
  page,
}) => {
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  await page.goto("/?fixture=current", { waitUntil: "networkidle" });

  await page
    .getByRole("button", {
      name: "Copy current user identity Fabien#TG-7K4P9D",
    })
    .click();

  await expect(page.getByText("Copied", { exact: true })).toBeVisible();
  await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toBe(
    "Fabien#TG-7K4P9D",
  );
  await expect(page.getByText("Copied", { exact: true })).toBeHidden({
    timeout: 2_500,
  });
});
