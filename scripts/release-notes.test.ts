import { describe, expect, test } from "vitest";

import {
  createReleaseNotes,
  previousStableTag,
  releaseChangesFromCommits,
  updaterReleaseNotes,
} from "./release-notes";

describe("release notes", () => {
  test("prefers reviewed release-note trailers and falls back to user-facing commits", () => {
    expect(
      releaseChangesFromCommits([
        {
          body: "Release-note: The Codex panel no longer shows the internal reserve limit.",
          subject: "fix(codex): hide reserve quota lane",
        },
        {
          body: "",
          subject: "perf(desktop): cache and prefetch Doomerboard queries (#98)",
        },
        { body: "", subject: "fix(release): rotate fixture candidate" },
        { body: "", subject: "chore(release): prepare database fixture" },
        { body: "", subject: "fix(dev): select local environment" },
      ]),
    ).toEqual([
      "The Codex panel no longer shows the internal reserve limit.",
      "Cache and prefetch Doomerboard queries.",
    ]);
  });

  test("keeps every reviewed note from a multi-change squash commit", () => {
    expect(
      releaseChangesFromCommits([
        {
          body: [
            "Release-note: Price the new Claude models.",
            "Release-note: Complete usage refreshes after the latest update starts.",
          ].join("\n"),
          subject: "feat(pricing): support new Claude models",
        },
      ]),
    ).toEqual([
      "Price the new Claude models.",
      "Complete usage refreshes after the latest update starts.",
    ]);
  });

  test("selects the previous stable tag and ignores prereleases", () => {
    expect(previousStableTag("v1.2.0", ["v1.0.0", "v1.1.0", "v1.2.0-beta.1", "v1.2.0"])).toBe(
      "v1.1.0",
    );
  });

  test("puts changes first and keeps verification details out of the main message", () => {
    const notes = createReleaseNotes(
      {
        changes: ["Fixed provider refresh."],
        previousTag: "v1.1.0",
        tag: "v1.2.0",
      },
      [
        {
          bytes: 123,
          name: "TouchGrassBar_1.2.0_aarch64.dmg",
          sha256: "a".repeat(64),
        },
      ],
    );

    expect(notes).toMatch(/^## What changed\n\n- Fixed provider refresh\./u);
    expect(notes).toContain("<summary>Technical verification</summary>");
    expect(notes).toContain("[v1.1.0...v1.2.0]");
    expect(notes).not.toContain("This Release is a draft");
    expect(updaterReleaseNotes(["Fixed provider refresh."])).toBe("Fixed provider refresh.");
  });
});
