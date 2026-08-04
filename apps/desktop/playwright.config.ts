import { defineConfig } from "@playwright/test";

const previewUrl = "http://127.0.0.1:1431";

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
    channel: "chrome",
    colorScheme: "light",
    deviceScaleFactor: 1,
    locale: "en-US",
    viewport: { height: 900, width: 520 },
  },
  webServer: {
    command: "bunx --bun vite --host 127.0.0.1 --port 1431",
    reuseExistingServer: false,
    timeout: 120_000,
    url: previewUrl,
  },
});
