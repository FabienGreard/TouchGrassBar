import { defineConfig } from "@playwright/test";

const previewPort = process.env.TOUCHGRASS_VISUAL_PORT ?? "1431";
const previewUrl = `http://127.0.0.1:${previewPort}`;

export default defineConfig({
  expect: {
    toHaveScreenshot: {
      animations: "disabled",
      maxDiffPixelRatio: 0.015,
      threshold: 0.2,
    },
  },
  forbidOnly: Boolean(process.env.CI),
  reporter: process.env.CI ? "github" : "list",
  testDir: "./e2e",
  testMatch: "**/*.pw.ts",
  use: {
    baseURL: previewUrl,
    colorScheme: "light",
    deviceScaleFactor: 1,
    locale: "en-US",
    viewport: { height: 900, width: 520 },
  },
  webServer: {
    command: `bunx --bun vite --host 127.0.0.1 --port ${previewPort}`,
    reuseExistingServer: !process.env.CI,
    timeout: 120_000,
    url: previewUrl,
  },
});
