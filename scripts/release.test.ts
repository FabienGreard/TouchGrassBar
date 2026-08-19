import { describe, expect, test } from "vitest";

import {
  bumpStableVersion,
  latestStableTag,
  parseReleaseArguments,
  parseStableVersion,
} from "./release";

describe("release command", () => {
  test("calculates patch, minor, and major releases", () => {
    const current = parseStableVersion("v1.2.3");
    expect(current).not.toBeNull();
    expect(bumpStableVersion(current!, "patch")).toBe("v1.2.4");
    expect(bumpStableVersion(current!, "minor")).toBe("v1.3.0");
    expect(bumpStableVersion(current!, "major")).toBe("v2.0.0");
  });

  test("uses the highest stable remote tag", () => {
    expect(
      latestStableTag(["v0.9.9", "v1.0.0-beta.1", "v1.0.0", "other"]),
    ).toMatchObject({ tag: "v1.0.0" });
  });

  test("uses preview mode by default", () => {
    expect(parseReleaseArguments(["patch"])).toEqual({
      execute: false,
      level: "patch",
    });
  });

  test("requires an explicit valid execution request", () => {
    expect(parseReleaseArguments(["minor", "--execute"])).toEqual({
      execute: true,
      level: "minor",
    });
    for (const argumentsList of [
      [],
      ["feature"],
      ["patch", "--unknown"],
      ["patch", "--execute", "--execute"],
    ]) {
      expect(() => parseReleaseArguments(argumentsList)).toThrow(
        "Use: bun run release",
      );
    }
  });
});
