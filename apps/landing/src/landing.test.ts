import { describe, expect, test } from "vitest";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { readFileSync } from "node:fs";

import LandingExperience from "./components/LandingExperience";
import {
  approvedDownloadFromRelease,
  downloadFallbackUrl,
  resolveApprovedDownload,
} from "./lib/download-resolver";
import { gardenTimeForHour } from "./lib/garden-time";
import { installInviteParallax } from "./lib/invite-parallax";
import { installWebMcp } from "./lib/webmcp";

function releaseAsset(name: string, tag = "v1.2.3") {
  return {
    browser_download_url: `https://github.com/FabienGreard/TouchGrassBar/releases/download/${tag}/${name}`,
    name,
    state: "uploaded",
  };
}

function fullRelease(overrides: Record<string, unknown> = {}) {
  const version = "1.2.3";
  const updaterArchive = `TouchGrassBar_${version}_aarch64.app.tar.gz`;
  return {
    assets: [
      releaseAsset("latest.json"),
      releaseAsset(`release-trust-${version}.json`),
      releaseAsset("SHA256SUMS"),
      releaseAsset(`TouchGrassBar_${version}_aarch64.dmg`),
      releaseAsset(updaterArchive),
      releaseAsset(`${updaterArchive}.sig`),
    ],
    draft: false,
    immutable: true,
    prerelease: false,
    published_at: "2026-08-07T12:00:00.000Z",
    tag_name: "v1.2.3",
    ...overrides,
  };
}

describe("production landing contract", () => {
  test("uses the approved night headline", () => {
    const markup = renderToStaticMarkup(
      createElement(LandingExperience, { initialGardenTime: "night" }),
    );

    expect(markup).toContain("Nothing good");
    expect(markup).toContain("Gets deployed");
    expect(markup).toContain("At this hour.");
  });

  test("uses the current desktop UI wording in product previews", () => {
    const markup = renderToStaticMarkup(
      createElement(LandingExperience, { initialGardenTime: "day" }),
    );

    for (const copy of [
      "Live",
      "Usage",
      "Most used",
      "GPT 5.6 Sol",
      "Leaderboard",
      "Doomerboard",
      "Prompts and provider data stay on your Mac",
      "See who burned the most tokens and who still remembers daylight.",
      "PROMPT ENJOYER",
      "nora",
      "Friends",
      "Codex and Claude, detected locally on your Mac.",
      "Vibe code alone.",
      "Tokenmaxx together.",
      "Add your friends.",
      "Compare token usage.",
      "Public Token Usage",
      "≈ $214.96",
      "Lives in your menu bar. See your limits and compare your usage on the leaderboard.",
      "Download for macOS",
      "Drag to install",
      "Applications",
    ]) {
      expect(markup).toContain(copy);
    }
    expect(markup).not.toContain("Step 1 of 3");
    expect(markup).not.toContain("Privacy boundary");
    expect(markup).not.toContain("No browser account.");
    expect(markup).not.toContain("Token Score");
  });

  test("marks conversion and outbound links for analytics", () => {
    const markup = renderToStaticMarkup(
      createElement(LandingExperience, { initialGardenTime: "day" }),
    );

    for (const placement of ["header", "hero", "invite", "release"]) {
      expect(markup).toContain(
        `data-analytics-event="download clicked" data-analytics-placement="${placement}"`,
      );
    }
    for (const placement of ["github", "x"]) {
      expect(markup).toContain(
        `data-analytics-event="outbound link clicked" data-analytics-placement="${placement}"`,
      );
    }
  });

  test("publishes canonical SEO and AI discovery files", () => {
    const landingSource = readFileSync(
      new URL("./components/LandingPage.astro", import.meta.url),
      "utf8",
    );
    const llmsText = readFileSync(new URL("../public/llms.txt", import.meta.url), "utf8");
    const robotsText = readFileSync(new URL("../public/robots.txt", import.meta.url), "utf8");
    const sitemapText = readFileSync(new URL("../public/sitemap.xml", import.meta.url), "utf8");

    expect(landingSource).toContain('rel="canonical"');
    expect(landingSource).toContain(
      'const title = "TouchGrassBar — Codex & Claude Usage in Your Menu Bar"',
    );
    expect(landingSource).toContain(
      "See your Codex and Claude usage limits in your Mac menu bar. Compare your usage on the leaderboard",
    );
    expect(landingSource).toContain('property="og:image"');
    expect(landingSource).toContain('new URL("/og.jpg", siteUrl)');
    expect(landingSource).toContain('content="image/jpeg"');
    expect(landingSource).toContain('name="twitter:card"');
    expect(landingSource).toContain('type="application/ld+json"');
    expect(llmsText).toMatch(/^# TouchGrassBar\n/);
    expect(llmsText).toContain("\n> TouchGrassBar is an open-source macOS menu bar app");
    expect(llmsText.match(/^## /gm)).toEqual(["## "]);
    expect(llmsText).toContain("## Primary links");
    expect(llmsText).not.toContain("Token Score");
    expect(robotsText).toContain("Sitemap: https://touchgrassbar.com/sitemap.xml");
    expect(sitemapText).toContain("<loc>https://touchgrassbar.com/</loc>");
  });

  test("registers a valid WebMCP download tool when the browser supports it", async () => {
    let clickCount = 0;
    let registeredTool:
      | {
          execute: () => { content: Array<{ text: string; type: string }> };
          inputSchema: unknown;
          name: string;
        }
      | undefined;
    const documentObject = {
      modelContext: {
        registerTool(tool: NonNullable<typeof registeredTool>) {
          registeredTool = tool;
        },
      },
      querySelector() {
        return { click: () => clickCount++ };
      },
    } as unknown as Document;

    expect(await installWebMcp(documentObject)).toBe(true);
    expect(registeredTool).toMatchObject({
      inputSchema: {
        additionalProperties: false,
        properties: {},
        type: "object",
      },
      name: "download-touchgrassbar-for-macos",
    });
    expect(registeredTool?.execute().content[0]?.type).toBe("text");
    expect(clickCount).toBe(1);
  });

  test("lazy-loads only the below-fold site brand image", () => {
    const markup = renderToStaticMarkup(
      createElement(LandingExperience, { initialGardenTime: "day" }),
    );
    const siteBrandImages = markup.match(
      /<div[^>]*class="[^"]*site-brand site-brand--reversed"[^>]*><img[^>]*data-slot="brand-mark"[^>]*>/g,
    );

    expect(siteBrandImages).toHaveLength(2);
    expect(siteBrandImages?.[0]).not.toContain('loading="lazy"');
    expect(siteBrandImages?.[1]).toContain('loading="lazy"');
  });

  test("maps each local hour to the approved garden scene", () => {
    expect(Array.from({ length: 24 }, (_, hour) => gardenTimeForHour(hour))).toEqual([
      "night",
      "night",
      "night",
      "night",
      "night",
      "dawn",
      "dawn",
      "dawn",
      "dawn",
      "day",
      "day",
      "day",
      "day",
      "day",
      "day",
      "day",
      "day",
      "golden",
      "golden",
      "golden",
      "golden",
      "night",
      "night",
      "night",
    ]);
    expect(() => gardenTimeForHour(24)).toThrow(
      "The local hour must be an integer from 0 through 23.",
    );
  });

  test("does not install invite parallax work on mobile", () => {
    const viewportListeners = new Set<string>();
    let animationFrameRequested = false;
    const noticeLayer = {
      querySelectorAll: () => [],
    };
    const section = {
      querySelector: () => noticeLayer,
    };
    const documentObject = {
      querySelector: () => section,
    } as unknown as Document;
    const windowObject = {
      addEventListener(type: string) {
        viewportListeners.add(type);
      },
      cancelAnimationFrame() {},
      matchMedia: () => ({
        addEventListener() {},
        matches: false,
        removeEventListener() {},
      }),
      removeEventListener(type: string) {
        viewportListeners.delete(type);
      },
      requestAnimationFrame() {
        animationFrameRequested = true;
        return 1;
      },
    } as unknown as Window;

    installInviteParallax(documentObject, windowObject);

    expect(animationFrameRequested).toBe(false);
    expect(viewportListeners.has("resize")).toBe(false);
    expect(viewportListeners.has("scroll")).toBe(false);
  });

  test("keeps invite parallax work on laptop screens", () => {
    const viewportListeners = new Set<string>();
    let animationFrameRequested = false;
    const documentObject = {
      querySelector: () => ({
        querySelector: () => ({ querySelectorAll: () => [] }),
      }),
    } as unknown as Document;
    const windowObject = {
      addEventListener(type: string) {
        viewportListeners.add(type);
      },
      cancelAnimationFrame() {},
      matchMedia: (query: string) => ({
        addEventListener() {},
        matches: query === "(min-width: 901px)",
        removeEventListener() {},
      }),
      removeEventListener(type: string) {
        viewportListeners.delete(type);
      },
      requestAnimationFrame() {
        animationFrameRequested = true;
        return 1;
      },
    } as unknown as Window;

    const removeParallax = installInviteParallax(documentObject, windowObject);

    expect(animationFrameRequested).toBe(true);
    expect(viewportListeners.has("resize")).toBe(true);
    expect(viewportListeners.has("scroll")).toBe(true);

    removeParallax();
    expect(viewportListeners.has("resize")).toBe(false);
    expect(viewportListeners.has("scroll")).toBe(false);
  });

  test("accepts only a complete immutable full release", async () => {
    expect(approvedDownloadFromRelease(fullRelease())).toEqual({
      url: "https://github.com/FabienGreard/TouchGrassBar/releases/download/v1.2.3/TouchGrassBar_1.2.3_aarch64.dmg",
      version: "1.2.3",
    });

    const completeAssets = fullRelease().assets;
    for (const release of [
      fullRelease({ draft: true }),
      fullRelease({ immutable: false }),
      fullRelease({ prerelease: true }),
      fullRelease({ tag_name: "v1.2.3-beta.1" }),
      fullRelease({ published_at: null }),
      fullRelease({ assets: completeAssets.slice(0, -1) }),
      fullRelease({
        assets: completeAssets.map((asset) =>
          asset.name.endsWith(".dmg")
            ? {
                browser_download_url: "https://example.com/app.dmg",
                name: asset.name,
                state: asset.state,
              }
            : asset,
        ),
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
  });
});
