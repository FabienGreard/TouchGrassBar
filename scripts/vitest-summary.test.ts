import { describe, expect, test } from "vitest";

import { parseVitestSummary, renderVitestSummary } from "./vitest-summary";

const report = `# Vitest Test Report
## Summary
**Test Files**: ✅ **28 passes** · 28 total
**Test Results**: ✅ **165 passes** · 165 total
**Other**: 1 skip · 1 total
`;

describe("Vitest job summary", () => {
  test("parses a GitHub Actions reporter section", () => {
    expect(parseVitestSummary("desktop", report)).toEqual({
      files: { failed: 0, passed: 28, total: 28 },
      id: "desktop",
      name: "Desktop",
      skipped: 1,
      tests: { failed: 0, passed: 165, total: 165 },
    });
  });

  test("renders labeled suites, totals, and the platform skip reason", () => {
    const desktop = parseVitestSummary("desktop", report);
    const contracts = parseVitestSummary(
      "contracts",
      report
        .replace("28 passes", "1 pass")
        .replace("28 total", "1 total")
        .replace("165 passes", "20 passes")
        .replace("165 total", "20 total")
        .replace("**Other**: 1 skip · 1 total\n", ""),
    );

    const rendered = renderVitestSummary(
      [desktop, contracts].filter((summary) => summary !== null),
    );
    expect(rendered).toContain("| Contracts | ✅ Passed | 1 / 1 | 20 / 20 | — |");
    expect(rendered).toContain("| Desktop | ✅ Passed | 28 / 28 | 165 / 165 | 1 |");
    expect(rendered).toContain(
      "| **Total** | **✅ Passed** | **29 / 29** | **185 / 185** | **1** |",
    );
    expect(rendered).toContain("macOS-only runner test on Ubuntu");
  });
});
