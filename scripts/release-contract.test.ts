import { describe, expect, test } from "vitest";

import {
  createLatestManifest,
  createPresenceReceipt,
  parseStableReleaseTag,
  releaseAssetNames,
  releaseEnvironmentVariables,
  releaseSecrets,
} from "./release-contract";

const hasValueField = (input: unknown): boolean =>
  typeof input === "object" && input !== null
    ? Object.entries(input).some(
        ([key, value]) => key === "value" || hasValueField(value),
      )
    : false;

const protectedStates = (present = true) =>
  Object.fromEntries(
    [...releaseSecrets, ...releaseEnvironmentVariables].map((name) => [
      name,
      present,
    ]),
  );

describe("release contract", () => {
  test("accepts only exact stable tags", () => {
    expect(parseStableReleaseTag("v1.2.3")).toEqual({
      tag: "v1.2.3",
      version: "1.2.3",
    });
    for (const tag of [
      "1.2.3",
      "v01.2.3",
      "v1.2.3-beta.1",
      "v1.2.3\nnext",
      "refs/tags/v1.2.3",
    ]) {
      expect(() => parseStableReleaseTag(tag)).toThrow(
        "Release tag must have exact vMAJOR.MINOR.PATCH form.",
      );
    }
  });

  test("records presence without protected values", () => {
    const receipt = createPresenceReceipt({
      capturedAt: "2026-08-07T12:00:00.000Z",
      commit: "a".repeat(40),
      protectedStates: protectedStates(),
      publicConfiguration: {
        TAURI_UPDATER_ENDPOINT: true,
        TAURI_UPDATER_PUBLIC_KEY: true,
      },
      tag: "v1.2.3",
      workflowRunId: "12345",
    });

    expect(receipt.redaction).toEqual({
      protected_values_emitted: false,
      protected_values_received: false,
    });
    expect(receipt.environments[0]?.secrets).toEqual(
      releaseSecrets.map((name) => ({
        name,
        scope: "environment:macos-release",
        state: "present",
      })),
    );
    expect(hasValueField(receipt)).toBe(false);
  });

  test("fails closed when release configuration is absent", () => {
    expect(() =>
      createPresenceReceipt({
        capturedAt: "2026-08-07T12:00:00.000Z",
        commit: "b".repeat(40),
        protectedStates: protectedStates(false),
        publicConfiguration: {
          TAURI_UPDATER_ENDPOINT: true,
          TAURI_UPDATER_PUBLIC_KEY: false,
        },
        tag: "v1.2.3",
        workflowRunId: "12345",
      }),
    ).toThrow("Release configuration is absent:");
  });

  test("uses versioned assets and one stable updater manifest", () => {
    const names = releaseAssetNames("v1.2.3");
    expect(names.dmg).toBe("TouchGrassBar_1.2.3_aarch64.dmg");
    expect(names.updaterArchive).toBe(
      "TouchGrassBar_1.2.3_aarch64.app.tar.gz",
    );
    expect(
      createLatestManifest({
        notes: "TouchGrassBar 1.2.3",
        pubDate: "2026-08-07T12:00:00.000Z",
        signature: "trusted updater signature",
        tag: "v1.2.3",
        updaterArchiveName: names.updaterArchive,
      }),
    ).toMatchObject({
      platforms: {
        "darwin-aarch64": {
          url: `https://github.com/FabienGreard/TouchGrassBar/releases/download/v1.2.3/${names.updaterArchive}`,
        },
      },
      version: "1.2.3",
    });
  });
});
