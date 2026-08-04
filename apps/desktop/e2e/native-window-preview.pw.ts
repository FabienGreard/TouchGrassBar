import { expect, test } from "@playwright/test";

const previewViewport = { height: 800, width: 1280 } as const;
const nativeWindowSize = { height: 580, width: 760 } as const;

for (const surface of ["onboarding", "settings"] as const) {
  test(`${surface} web preview keeps the native window dimensions`, async ({
    page,
  }) => {
    await page.setViewportSize(previewViewport);
    await page.goto(`/?window=${surface}&fixture=current`, {
      waitUntil: "networkidle",
    });

    const nativeWindow = page.locator('[data-slot="native-window"]');
    await expect(nativeWindow).toBeVisible();

    const layout = await page.evaluate(() => {
      const element = document.querySelector<HTMLElement>(
        '[data-slot="native-window"]',
      );
      if (!element) throw new Error("Native window preview is missing");

      const { height, width } = element.getBoundingClientRect();
      return {
        documentClientHeight: document.documentElement.clientHeight,
        documentScrollHeight: document.documentElement.scrollHeight,
        height: Math.round(height),
        width: Math.round(width),
      };
    });

    expect(layout.width).toBe(nativeWindowSize.width);
    expect(layout.height).toBe(nativeWindowSize.height);
    expect(layout.documentScrollHeight).toBe(layout.documentClientHeight);
  });
}
