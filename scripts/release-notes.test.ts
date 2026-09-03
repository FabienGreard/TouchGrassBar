import { describe, expect, test } from "vitest";

import {
  createReleaseNotes,
  previousStableTag,
  releaseChangesFromCommits,
  releaseHistoryArguments,
  releaseTrailersFromBody,
  selectEquivalentReleaseCommit,
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

  test("uses one reviewed replacement summary for an uneditable squash commit", () => {
    expect(
      releaseChangesFromCommits([
        {
          body: "",
          subject: "fix(providers): fail closed on payload shape, not provider identity",
        },
        {
          body: [
            "Release-note-mode: replace",
            "Release-note: Claude usage remains visible after compatible Claude Code updates.",
            "Release-note: Provider quota lanes tolerate unrelated response fields.",
          ].join("\n"),
          subject: "chore(release): prepare database fixture",
        },
      ]),
    ).toEqual([
      "Claude usage remains visible after compatible Claude Code updates.",
      "Provider quota lanes tolerate unrelated response fields.",
    ]);
  });

  test("keeps user-facing changes that follow a replacement summary", () => {
    expect(
      releaseChangesFromCommits([
        {
          body: "Release-note: An earlier note.",
          subject: "fix(desktop): repair earlier behavior",
        },
        {
          body: "Release-note-mode: replace\nRelease-note: The reviewed replacement.",
          subject: "docs(release): replace earlier notes",
        },
        {
          body: "Release-note: A later reviewed note.",
          subject: "fix(claude): repair later behavior",
        },
        {
          body: "",
          subject: "feat(desktop): add a later feature",
        },
      ]),
    ).toEqual(["The reviewed replacement.", "A later reviewed note.", "Add a later feature."]);
  });

  test("requires an explicit user-facing trailer in a replacement summary", () => {
    expect(() =>
      releaseChangesFromCommits([
        {
          body: "Release-note-mode: replace",
          subject: "fix(providers): use a technical fallback subject",
        },
      ]),
    ).toThrow("The replacement release summary has no user-facing note.");
  });

  test("does not read the line after an empty replacement note", () => {
    expect(() =>
      releaseChangesFromCommits([
        {
          body: [
            "Release-note-mode: replace",
            "Release-note:",
            "Co-authored-by: Example <example@example.com>",
          ].join("\n"),
          subject: "fix(providers): use a technical fallback subject",
        },
      ]),
    ).toThrow("The replacement release summary has no user-facing note.");
  });

  test("ignores release markers in nested squash text", () => {
    const body = [
      "Nested commit message:",
      "Release-note-mode: replace",
      "Release-note: A nested note that must not replace the summary.",
      "",
      "The top-level squash summary continues here.",
      "",
      "Release-note: The reviewed top-level note.",
    ].join("\n");

    expect(releaseTrailersFromBody(body)).toEqual({
      modes: [],
      notes: ["The reviewed top-level note."],
    });
    expect(
      releaseChangesFromCommits([
        {
          body,
          subject: "fix(release): keep only top-level release trailers",
        },
      ]),
    ).toEqual(["The reviewed top-level note."]);
  });

  test("ignores a nested release trailer block at the end of a squash body", () => {
    expect(
      releaseChangesFromCommits([
        {
          body: "Release-note: Keep the earlier reviewed note.",
          subject: "fix(desktop): keep the reviewed behavior",
        },
        {
          body: [
            "Squashed commit contains:",
            "fix(nested): describe the nested change",
            "",
            "Release-note-mode: replace",
            "Release-note: Nested note.",
          ].join("\n"),
          subject: "chore(release): collect squashed work",
        },
      ]),
    ).toEqual(["Keep the earlier reviewed note."]);
  });

  test("ignores a compact nested release trailer block at the end of a squash body", () => {
    expect(
      releaseChangesFromCommits([
        {
          body: "Release-note: Keep the earlier reviewed note.",
          subject: "fix(desktop): keep the reviewed behavior",
        },
        {
          body: [
            "Squashed commit contains:",
            "",
            "fix: describe the nested change",
            "Release-note-mode: replace",
            "Release-note: Nested note.",
          ].join("\n"),
          subject: "chore(release): collect squashed work",
        },
      ]),
    ).toEqual(["Keep the earlier reviewed note."]);
  });

  test("ignores an unscoped custom subject in a compact nested trailer block", () => {
    expect(
      releaseChangesFromCommits([
        {
          body: "Release-note: Keep the earlier reviewed note.",
          subject: "fix(desktop): keep the reviewed behavior",
        },
        {
          body: [
            "Squashed commit contains:",
            "",
            "security: describe the nested change",
            "Release-note-mode: replace",
            "Release-note: Nested note.",
          ].join("\n"),
          subject: "chore(release): collect squashed work",
        },
      ]),
    ).toEqual(["Keep the earlier reviewed note."]);
  });

  test("ignores an unscoped custom subject in earlier squash text", () => {
    expect(
      releaseChangesFromCommits([
        {
          body: "Release-note: Keep the earlier reviewed note.",
          subject: "fix(desktop): keep the reviewed behavior",
        },
        {
          body: [
            "security: describe the nested change",
            "",
            "Release-note-mode: replace",
            "Release-note: Nested note.",
          ].join("\n"),
          subject: "chore(release): collect squashed work",
        },
      ]),
    ).toEqual(["Keep the earlier reviewed note."]);
  });

  test("ignores a hash-prefixed nested subject in earlier squash text", () => {
    expect(
      releaseChangesFromCommits([
        {
          body: "Release-note: Keep the earlier reviewed note.",
          subject: "fix(desktop): keep the reviewed behavior",
        },
        {
          body: [
            "abc1234 fix(parser): describe the nested change",
            "",
            "Release-note-mode: replace",
            "Release-note: Nested note.",
          ].join("\n"),
          subject: "chore(release): collect squashed work",
        },
      ]),
    ).toEqual(["Keep the earlier reviewed note."]);
  });

  test("ignores a decorated oneline subject in earlier squash text", () => {
    expect(
      releaseChangesFromCommits([
        {
          body: "Release-note: Keep the earlier reviewed note.",
          subject: "fix(desktop): keep the reviewed behavior",
        },
        {
          body: [
            "abc1234 (HEAD -> main, tag: v1.0.0) fix(parser): describe the nested change",
            "",
            "Release-note-mode: replace",
            "Release-note: Nested note.",
          ].join("\n"),
          subject: "chore(release): collect squashed work",
        },
      ]),
    ).toEqual(["Keep the earlier reviewed note."]);
  });

  test("ignores an uppercase nested subject in earlier squash text", () => {
    expect(
      releaseChangesFromCommits([
        {
          body: "Release-note: Keep the earlier reviewed note.",
          subject: "fix(desktop): keep the reviewed behavior",
        },
        {
          body: [
            "Fix(parser): describe the nested change",
            "",
            "Release-note-mode: replace",
            "Release-note: Nested note.",
          ].join("\n"),
          subject: "chore(release): collect squashed work",
        },
      ]),
    ).toEqual(["Keep the earlier reviewed note."]);
  });

  test.each([
    "note(parser): describe the nested change",
    "status(parser): describe the nested change",
    "note!: describe the nested change",
    "status!: describe the nested change",
  ])("ignores a scoped or breaking nested body subject: %s", (nestedSubject) => {
    expect(
      releaseChangesFromCommits([
        {
          body: "Release-note: Keep the earlier reviewed note.",
          subject: "fix(desktop): keep the reviewed behavior",
        },
        {
          body: [
            nestedSubject,
            "",
            "Release-note-mode: replace",
            "Release-note: Nested note.",
          ].join("\n"),
          subject: "chore(release): collect squashed work",
        },
      ]),
    ).toEqual(["Keep the earlier reviewed note."]);
  });

  test.each(["co-authored-by", "reviewed-by", "signed-off-by", "tested-by"])(
    "keeps a lower-case standard Git trailer in the final block: %s",
    (token) => {
      expect(
        releaseTrailersFromBody(
          [
            "Release-note-mode: replace",
            "Release-note: Keep this reviewed note.",
            `${token}: Example <example@example.com>`,
          ].join("\n"),
        ),
      ).toEqual({
        modes: ["replace"],
        notes: ["Keep this reviewed note."],
      });
    },
  );

  test.each(["Co-authored-by", "Reviewed-by", "Signed-off-by", "Tested-by"])(
    "keeps a capitalized standard Git trailer in the final block: %s",
    (token) => {
      expect(
        releaseTrailersFromBody(
          [
            "Release-note-mode: replace",
            "Release-note: Keep this reviewed note.",
            `${token}: Example <example@example.com>`,
          ].join("\n"),
        ),
      ).toEqual({
        modes: ["replace"],
        notes: ["Keep this reviewed note."],
      });
    },
  );

  test("treats an adjacent unknown lower-case field as ambiguous", () => {
    expect(
      releaseTrailersFromBody(
        ["status: active", "Release-note: Keep this reviewed note."].join("\n"),
      ),
    ).toEqual({ modes: [], notes: [] });
  });

  test.each([
    "fix: describe the nested change",
    "  * fix(parser): describe the nested change",
    "1. fix(parser): describe the nested change",
    "> fix(parser): describe the nested change",
    ">fix(parser): describe the nested change",
    ">>fix(parser): describe the nested change",
    "### fix(parser): describe the nested change",
    "`fix(parser): describe the nested change`",
    "``fix(parser): describe the nested change``",
    "**fix(parser): describe the nested change**",
    "_**fix(parser): describe the nested change**_",
    "~~fix(parser): describe the nested change~~",
    "[fix(parser): describe the nested change](https://example.com)",
    "| fix(parser): describe the nested change |",
    "| **fix(parser): describe the nested change** |",
    "| abc123 | fix(parser): describe the nested change |",
    "| &#92;| fix(parser): describe the nested change |",
    "<code>fix(parser): describe the nested change</code>",
    '<span title=">">fix(parser): describe the nested change</span>',
    '<span title="&quot;">fix(parser): describe the nested change</span>',
    "<!-- marker -->fix(parser): describe the nested change",
    "<?marker?>fix(parser): describe the nested change",
    "<!DOCTYPE html>fix(parser): describe the nested change",
    "<![CDATA[marker]]>fix(parser): describe the nested change",
    "&nbsp;fix(parser): describe the nested change",
    "&#32;fix(parser): describe the nested change",
    "&#x66;ix(parser): describe the nested change",
    "f&#105;x(parser): describe the nested change",
    "fix(parser)&#58; describe the nested change",
    "security(parser): describe the nested change",
    "&ast;&ast;fix(parser): describe the nested change&ast;&ast;",
    "&vert; fix(parser): describe the nested change &vert;",
    "**fix**(parser): describe the nested change",
    "<strong>fix</strong>(parser): describe the nested change",
    "[fix](https://example.com)(parser): describe the nested change",
  ])("does not parse a prefixed nested subject as a trailer: %s", (nestedSubject) => {
    expect(
      releaseChangesFromCommits([
        {
          body: "Release-note: Keep the earlier reviewed note.",
          subject: "fix(desktop): keep the reviewed behavior",
        },
        {
          body: [
            nestedSubject,
            "",
            "Release-note-mode: replace",
            "Release-note: Nested note.",
          ].join("\n"),
          subject: "chore(release): collect squashed work",
        },
      ]),
    ).toEqual(["Keep the earlier reviewed note."]);
  });

  test("does not parse a nested subject from a GFM table without outer pipes", () => {
    expect(
      releaseChangesFromCommits([
        {
          body: "Release-note: Keep the earlier reviewed note.",
          subject: "fix(desktop): keep the reviewed behavior",
        },
        {
          body: [
            "Change | Detail",
            "--- | ---",
            "abc123 | fix(parser): describe the nested change",
            "",
            "Release-note-mode: replace",
            "Release-note: Nested note.",
          ].join("\n"),
          subject: "chore(release): collect squashed work",
        },
      ]),
    ).toEqual(["Keep the earlier reviewed note."]);
  });

  test("does not parse a nested subject from a quoted GFM table", () => {
    expect(
      releaseChangesFromCommits([
        {
          body: "Release-note: Keep the earlier reviewed note.",
          subject: "fix(desktop): keep the reviewed behavior",
        },
        {
          body: [
            "> Change | Detail",
            "> --- | ---",
            "> abc123 | fix(parser): describe the nested change",
            "",
            "Release-note-mode: replace",
            "Release-note: Nested note.",
          ].join("\n"),
          subject: "chore(release): collect squashed work",
        },
      ]),
    ).toEqual(["Keep the earlier reviewed note."]);
  });

  test.each([
    "@fix(parser): ask the owner",
    "(fix(parser): explanatory text",
    "value | fix(parser): explanatory text",
    String.raw`value \| fix(parser): explanatory text`,
    "status: active",
    "note: explain the parser behavior",
    ["```yaml", "status: active", "```"].join("\n"),
    ["Header only", "--- | ---", "value | fix(parser): explanatory text"].join("\n"),
  ])("keeps top-level trailers after non-Markdown punctuation: %s", (bodyLine) => {
    expect(
      releaseTrailersFromBody([bodyLine, "", "Release-note: Keep this note."].join("\n")),
    ).toEqual({
      modes: [],
      notes: ["Keep this note."],
    });
  });

  test("rejects more than one replacement summary", () => {
    expect(() =>
      releaseChangesFromCommits([
        {
          body: "Release-note-mode: replace\nRelease-note: First summary.",
          subject: "docs(release): add first summary",
        },
        {
          body: "Release-note-mode: replace\nRelease-note: Second summary.",
          subject: "docs(release): add second summary",
        },
      ]),
    ).toThrow("Release notes have more than one replacement summary.");
  });

  test("collects the normal range after a resolved release baseline", () => {
    expect(releaseHistoryArguments("rewritten-release", "candidate")).toEqual([
      "log",
      "--first-parent",
      "--reverse",
      "--format=%s%x00%b%x1e",
      "rewritten-release..candidate",
    ]);
  });

  test("finds one tree-and-subject equivalent after a metadata rewrite", () => {
    const tagged = {
      commit: "a".repeat(40),
      subject: "chore(release): prepare fixture",
      tree: "b".repeat(40),
    };
    expect(
      selectEquivalentReleaseCommit(tagged, [
        {
          commit: "c".repeat(40),
          subject: "fix(desktop): later change",
          tree: "d".repeat(40),
        },
        {
          commit: "e".repeat(40),
          subject: tagged.subject,
          tree: tagged.tree,
        },
      ]),
    ).toBe("e".repeat(40));
  });

  test("rejects an ambiguous rewritten release baseline", () => {
    const tagged = {
      commit: "a".repeat(40),
      subject: "chore(release): prepare fixture",
      tree: "b".repeat(40),
    };
    expect(() =>
      selectEquivalentReleaseCommit(tagged, [
        { ...tagged, commit: "c".repeat(40) },
        { ...tagged, commit: "d".repeat(40) },
      ]),
    ).toThrow("Rewritten release baseline is not unique on the target history.");
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
        comparisonBaseline: "rewritten-release",
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
    expect(notes).toContain(
      "https://github.com/FabienGreard/TouchGrassBar/compare/rewritten-release...v1.2.0",
    );
    expect(notes).not.toContain(
      "https://github.com/FabienGreard/TouchGrassBar/compare/v1.1.0...v1.2.0",
    );
    expect(notes).not.toContain("This Release is a draft");
    expect(updaterReleaseNotes(["Fixed provider refresh."])).toBe("Fixed provider refresh.");
  });
});
