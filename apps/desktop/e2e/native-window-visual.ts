import { expect, type Page } from "@playwright/test";

const nativeWindowSize = { height: 580, width: 760 } as const;

async function openNativeWindowPreview(
  page: Page,
  url: string,
  navigationName: string,
) {
  await page.setViewportSize(nativeWindowSize);
  await page.emulateMedia({ colorScheme: "light", reducedMotion: "reduce" });
  await page.goto(url, { waitUntil: "networkidle" });
  await page.evaluate(() => document.fonts.ready);

  await expect(page.locator('[data-slot="native-window"]')).toBeVisible();
  await expect(page.locator('[data-slot="brand"]')).toBeVisible();
  await expect(
    page.getByRole("navigation", { name: navigationName }),
  ).toBeVisible();
}

async function captureNativeWindow(page: Page, snapshotName: string) {
  const previewControls = page.locator(
    '[data-slot="dev-preview-switcher"], [data-slot="dev-preview-restore"]',
  );

  await expect(previewControls).toHaveCount(1);
  await page.evaluate(() => {
    delete document.documentElement.dataset.desktopPreview;
    document
      .querySelectorAll(
        '[data-slot="dev-preview-switcher"], [data-slot="dev-preview-restore"]',
      )
      .forEach((element) => element.remove());
  });
  await expect(previewControls).toHaveCount(0);

  const nativeWindow = page.locator('[data-slot="native-window"]');
  await expect
    .poll(() =>
      nativeWindow.evaluate((element) => {
        const { height, width, x, y } = element.getBoundingClientRect();
        return [x, y, width, height].map(Math.round);
      }),
    )
    .toEqual([0, 0, nativeWindowSize.width, nativeWindowSize.height]);

  await expect(page).toHaveScreenshot(snapshotName);
}

export { captureNativeWindow, openNativeWindowPreview };
