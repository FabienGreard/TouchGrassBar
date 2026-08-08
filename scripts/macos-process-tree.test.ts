import { describe, expect, test } from "vitest";

import {
  parseMacosProcessTable,
  sumMacosProcessTree,
} from "./macos-process-tree";

describe("macOS process-tree measurements", () => {
  test("parses the dependency-free ps output format", () => {
    expect(
      parseMacosProcessTable(`
          100     1   0.5   1024
          101   100  12.25  2048
      `),
    ).toEqual([
      { cpuPercent: 0.5, parentPid: 1, pid: 100, rssKilobytes: 1024 },
      { cpuPercent: 12.25, parentPid: 100, pid: 101, rssKilobytes: 2048 },
    ]);
  });

  test("sums the root and all nested helpers only", () => {
    const records = parseMacosProcessTable(`
      100 1   1.5 100
      101 100 2.0 200
      102 101 0.5 300
      900 1  80.0 900
      901 900 20.0 901
    `);

    expect(sumMacosProcessTree(records, 100)).toEqual({
      cpuPercent: 4,
      rssBytes: 600 * 1024,
    });
  });

  test("fails when the root process is absent", () => {
    const records = parseMacosProcessTable("101 100 1.0 512");

    expect(() => sumMacosProcessTree(records, 100)).toThrow(
      "Process-tree root is absent.",
    );
  });

  test.each([
    ["missing field", "100 1 0.5"],
    ["extra field", "100 1 0.5 1024 extra"],
    ["zero PID", "0 1 0.5 1024"],
    ["negative PID", "-100 1 0.5 1024"],
    ["negative parent PID", "100 -1 0.5 1024"],
    ["nonfinite CPU", "100 1 Infinity 1024"],
    ["not-a-number CPU", "100 1 NaN 1024"],
    ["negative CPU", "100 1 -0.5 1024"],
    ["fractional RSS", "100 1 0.5 1024.5"],
    ["negative RSS", "100 1 0.5 -1024"],
    ["unsafe PID", "9007199254740992 1 0.5 1024"],
    ["unsafe RSS", "100 1 0.5 9007199254740992"],
  ])("rejects a malformed record: %s", (_name, output) => {
    expect(() => parseMacosProcessTable(output)).toThrow(
      "macOS process record is malformed.",
    );
  });

  test("rejects duplicate process identifiers", () => {
    expect(() =>
      parseMacosProcessTable(`
        100 1 0.5 1024
        100 1 0.5 1024
      `),
    ).toThrow("macOS process record has a duplicate PID.");
  });

  test.each([0, -1, 1.5, Number.NaN, Number.POSITIVE_INFINITY])(
    "rejects an invalid root PID: %s",
    (rootPid) => {
      expect(() => sumMacosProcessTree([], rootPid)).toThrow(
        "Process-tree root PID is invalid.",
      );
    },
  );
});
