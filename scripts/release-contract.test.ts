import { describe, expect, test } from "vitest";

import {
  assertDatabaseFixtureReleaseSet,
  createDatabaseCompatibilityEvidence,
  createLatestManifest,
  createPresenceReceipt,
  parseDatabaseFixtureManifest,
  parseStableReleaseTag,
  releaseAssetNames,
  releaseEnvironmentVariables,
  releaseSecrets,
} from "./release-contract";

const databaseFixtureManifestEntries = [
  {
    database: "v0.0.8/touchgrassbar.sqlite3",
    releaseStatus: "official" as const,
    sha256: "8".repeat(64),
    sourceCommit: "8".repeat(40),
    tag: "v0.0.8",
  },
  {
    database: "v0.0.9/touchgrassbar.sqlite3",
    releaseStatus: "candidate" as const,
    sha256: "9".repeat(64),
    sourceCommit: "candidate",
    tag: "v0.0.9",
  },
];
const databaseFixtures = databaseFixtureManifestEntries.map(({ database, sha256, tag }) => ({
  database,
  sha256,
  tag,
}));

const hasValueField = (input: unknown): boolean =>
  typeof input === "object" && input !== null
    ? Object.entries(input).some(([key, value]) => key === "value" || hasValueField(value))
    : false;

const protectedStates = (present = true) =>
  Object.fromEntries(
    [...releaseSecrets, ...releaseEnvironmentVariables].map((name) => [name, present]),
  );

describe("release contract", () => {
  test("accepts only exact stable tags", () => {
    expect(parseStableReleaseTag("v1.2.3")).toEqual({
      tag: "v1.2.3",
      version: "1.2.3",
    });
    for (const tag of ["1.2.3", "v01.2.3", "v1.2.3-beta.1", "v1.2.3\nnext", "refs/tags/v1.2.3"]) {
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
    expect(names.databaseCompatibility).toBe("database-compatibility-1.2.3.json");
    expect(names.dmg).toBe("TouchGrassBar_1.2.3_aarch64.dmg");
    expect(names.updaterArchive).toBe("TouchGrassBar_1.2.3_aarch64.app.tar.gz");
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

  test("requires one safe fixture for the exact database candidate", () => {
    expect(
      parseDatabaseFixtureManifest(
        { fixtures: databaseFixtureManifestEntries, formatVersion: 1 },
        "v0.0.9",
      ),
    ).toEqual(databaseFixtureManifestEntries);
    expect(() =>
      parseDatabaseFixtureManifest(
        {
          fixtures: databaseFixtureManifestEntries.slice(0, 1),
          formatVersion: 1,
        },
        "v0.0.9",
      ),
    ).toThrow("Database fixture manifest must have only candidate v0.0.9.");
    expect(() =>
      parseDatabaseFixtureManifest(
        {
          fixtures: [
            databaseFixtureManifestEntries[1],
            {
              ...databaseFixtureManifestEntries[1],
              database: "v0.0.9/other.sqlite3",
            },
          ],
          formatVersion: 1,
        },
        "v0.0.9",
      ),
    ).toThrow("Database fixture manifest has a duplicate entry.");
    expect(() =>
      parseDatabaseFixtureManifest(
        {
          fixtures: [
            {
              ...databaseFixtureManifestEntries[1],
              database: "../private.sqlite3",
            },
          ],
          formatVersion: 1,
        },
        "v0.0.9",
      ),
    ).toThrow("Database fixture manifest entry is invalid.");
  });

  test("requires exact official releases and one exact candidate", () => {
    expect(() =>
      assertDatabaseFixtureReleaseSet(databaseFixtureManifestEntries, "v0.0.9", ["v0.0.8"]),
    ).not.toThrow();

    for (const publishedTags of [[], ["v0.0.7"], ["v0.0.8", "v0.0.8"]]) {
      expect(() =>
        assertDatabaseFixtureReleaseSet(databaseFixtureManifestEntries, "v0.0.9", publishedTags),
      ).toThrow("Official database fixtures do not match published stable GitHub Releases.");
    }

    expect(() =>
      assertDatabaseFixtureReleaseSet(databaseFixtureManifestEntries.slice(1), "v0.0.9", [
        "v0.0.8",
      ]),
    ).toThrow("Official database fixtures do not match published stable GitHub Releases.");

    expect(() =>
      assertDatabaseFixtureReleaseSet(
        databaseFixtureManifestEntries.map((fixture) => ({
          ...fixture,
          releaseStatus: "official" as const,
          sourceCommit: "a".repeat(40),
        })),
        "v0.0.9",
        ["v0.0.8", "v0.0.9"],
      ),
    ).toThrow("Database fixture manifest must have only candidate v0.0.9.");

    expect(() =>
      assertDatabaseFixtureReleaseSet(
        [
          databaseFixtureManifestEntries[0],
          databaseFixtureManifestEntries[1],
          {
            database: "v0.0.10/touchgrassbar.sqlite3",
            releaseStatus: "candidate",
            sha256: "a".repeat(64),
            sourceCommit: "candidate",
            tag: "v0.0.10",
          },
        ],
        "v0.0.9",
        ["v0.0.8"],
      ),
    ).toThrow("Database fixture manifest must have only candidate v0.0.9.");
  });

  test("binds database compatibility evidence to the release identity", () => {
    expect(
      createDatabaseCompatibilityEvidence({
        capturedAt: "2026-08-08T12:00:00.000Z",
        commit: "c".repeat(40),
        fixtures: databaseFixtures,
        tag: "v0.0.9",
        workflowRunId: "456",
      }),
    ).toMatchObject({
      candidate: {
        commit: "c".repeat(40),
        tag: "v0.0.9",
        workflow_run_id: "456",
      },
      redaction: {
        private_paths: "ABSENT",
        sensitive_values: "ABSENT",
      },
      schema_version: "touchgrass.database-compatibility.v1",
      verification: { result: "PASS" },
    });
    expect(
      createDatabaseCompatibilityEvidence({
        capturedAt: "2026-08-08T12:00:00.000Z",
        commit: "c".repeat(40),
        fixtures: databaseFixtures,
        tag: "v0.0.9",
        workflowRunId: "456",
      }).fixtures,
    ).toEqual(databaseFixtures.map((fixture) => ({ ...fixture, result: "PASS" })));
  });
});
