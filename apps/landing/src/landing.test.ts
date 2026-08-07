import { readFileSync } from "node:fs";

import { describe, expect, test } from "vitest";

import {
  approvedDownloadFromRelease,
  downloadFallbackUrl,
  latestReleaseApiUrl,
  resolveApprovedDownload,
} from "./lib/download-resolver";
import { gardenTimeForHour } from "./lib/garden-time";

const component = readFileSync(
  new URL("./components/LandingPage.astro", import.meta.url),
  "utf8",
);
const styles = readFileSync(
  new URL("./styles/global.css", import.meta.url),
  "utf8",
);
const invitationRoute = readFileSync(
  new URL("./pages/join.astro", import.meta.url),
  "utf8",
);
const normalizedComponent = component.replace(/\s+/g, " ");

function fullRelease(overrides: Record<string, unknown> = {}) {
  return {
    assets: [
      {
        browser_download_url:
          "https://github.com/FabienGreard/TouchGrassBar/releases/download/v1.2.3/TouchGrassBar_1.2.3_aarch64.dmg",
        name: "TouchGrassBar_1.2.3_aarch64.dmg",
        state: "uploaded",
      },
    ],
    draft: false,
    prerelease: false,
    published_at: "2026-08-07T12:00:00.000Z",
    tag_name: "v1.2.3",
    ...overrides,
  };
}

describe("production landing contract", () => {
  test("maps every local hour to one approved garden state", () => {
    expect(Array.from({ length: 24 }, (_, hour) => gardenTimeForHour(hour))).toEqual([
      "night", "night", "night", "night", "night",
      "dawn", "dawn", "dawn", "dawn",
      "day", "day", "day", "day", "day", "day", "day", "day",
      "golden", "golden", "golden", "golden",
      "night", "night", "night",
    ]);
    expect(() => gardenTimeForHour(24)).toThrow(
      "The local hour must be an integer from 0 through 23.",
    );
  });

  test("accepts only the exact versioned DMG from a full stable Release", async () => {
    expect(approvedDownloadFromRelease(fullRelease())).toEqual({
      url: "https://github.com/FabienGreard/TouchGrassBar/releases/download/v1.2.3/TouchGrassBar_1.2.3_aarch64.dmg",
      version: "1.2.3",
    });

    for (const release of [
      fullRelease({ draft: true }),
      fullRelease({ prerelease: true }),
      fullRelease({ tag_name: "v1.2.3-beta.1" }),
      fullRelease({ published_at: null }),
      fullRelease({ assets: [] }),
      fullRelease({
        assets: [
          {
            browser_download_url:
              "https://example.com/TouchGrassBar_1.2.3_aarch64.dmg",
            name: "TouchGrassBar_1.2.3_aarch64.dmg",
            state: "uploaded",
          },
        ],
      }),
    ]) {
      expect(approvedDownloadFromRelease(release)).toBeNull();
    }

    const fetchFailure = async () => ({
      json: async () => fullRelease(),
      ok: false,
    });
    expect(await resolveApprovedDownload(fetchFailure)).toBeNull();
    expect(downloadFallbackUrl).toBe(
      "https://github.com/FabienGreard/TouchGrassBar/releases/latest",
    );
    expect(
      latestReleaseApiUrl.endsWith(
        "/repos/FabienGreard/TouchGrassBar/releases/latest",
      ),
    ).toBe(true);
  });

  test("keeps the approved product, access, and visual contracts in one page", () => {
    for (const requiredCopy of [
      "macOS menu bar",
      "Codex and Claude",
      "Tokenmaxxers",
      "Doomerboard",
      "Daily Usage Aggregates",
      "Bootstrap",
      "signed updates",
      "no desktop dashboard",
      "no browser account",
      "no public Profile page",
    ]) {
      expect(normalizedComponent).toContain(requiredCopy);
    }
    expect(component).not.toMatch(/sign[ -]?in|browser dashboard/i);
    expect(component.match(/data-download-link/g)).toHaveLength(3);
    expect(component).toContain('href={downloadFallbackUrl}');
    expect(invitationRoute).toContain("<LandingPage invitation />");

    for (const visualContract of [
      ':root[data-garden-time="dawn"]',
      ':root[data-garden-time="day"]',
      ':root[data-garden-time="golden"]',
      ':root[data-garden-time="night"]',
      "min-height: 100svh",
      ".site-shell",
      ".brand__mark img",
      ".doomerboard-section::before",
      ":focus-visible",
      "@media (prefers-reduced-motion: reduce)",
      "@media (prefers-contrast: more)",
      "@media (max-width: 620px)",
    ]) {
      expect(styles).toContain(visualContract);
    }
  });

  test("keeps internal links on declared routes and sections", () => {
    for (const sectionId of [
      "main-content",
      "doomerboard",
      "privacy",
      "how-it-works",
      "download",
    ]) {
      expect(component).toContain(`id="${sectionId}"`);
    }
    expect(component).toContain('href="/join"');
    expect(component).toContain(
      'href="https://github.com/FabienGreard/TouchGrassBar"',
    );
  });
});
