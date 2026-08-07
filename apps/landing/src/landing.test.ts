import { describe, expect, test } from "vitest";

import {
  approvedDownloadFromRelease,
  downloadFallbackUrl,
  resolveApprovedDownload,
} from "./lib/download-resolver";
import { gardenTimeForHour } from "./lib/garden-time";

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
  test("maps each local hour to the approved garden scene", () => {
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
