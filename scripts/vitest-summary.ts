type Counts = {
  failed: number;
  passed: number;
  total: number;
};

export type VitestSuiteSummary = {
  files: Counts;
  id: string;
  name: string;
  skipped: number;
  tests: Counts;
};

const suiteDetails: Record<string, { name: string; order: number }> = {
  contracts: { name: "Contracts", order: 10 },
  landing: { name: "Landing page", order: 20 },
  desktop: { name: "Desktop", order: 30 },
  backend: { name: "Backend", order: 40 },
  release: { name: "Release governance", order: 50 },
  macos: { name: "macOS integration", order: 60 },
};

function reportLine(markdown: string, label: string): string | undefined {
  return markdown
    .split("\n")
    .find((line) => line.includes(`**${label}**`));
}

function count(line: string | undefined, resultPattern: string): number {
  if (!line) return 0;
  const match = new RegExp(`(\\d+) ${resultPattern}`, "iu").exec(line);
  return Number.parseInt(match?.[1] ?? "0", 10);
}

function counts(line: string): Counts {
  return {
    failed: count(line, "fail(?:s|ed)?"),
    passed: count(line, "pass(?:es|ed)?"),
    total: count(line, "total"),
  };
}

export function parseVitestSummary(
  id: string,
  markdown: string,
): VitestSuiteSummary | null {
  const fileLine = reportLine(markdown, "Test Files");
  const testLine = reportLine(markdown, "Test Results");
  if (!fileLine || !testLine) return null;

  const details = suiteDetails[id] ?? { name: id, order: 1_000 };
  return {
    files: counts(fileLine),
    id,
    name: details.name,
    skipped: count(reportLine(markdown, "Other"), "skip(?:s|ped)?"),
    tests: counts(testLine),
  };
}

function result(summary: VitestSuiteSummary): string {
  if (summary.files.failed > 0 || summary.tests.failed > 0) return "❌ Failed";
  return "✅ Passed";
}

function passed(countsValue: Counts): string {
  return `${countsValue.passed} / ${countsValue.total}`;
}

export function renderVitestSummary(
  summaries: VitestSuiteSummary[],
): string {
  if (summaries.length === 0) {
    return [
      "# Test report",
      "",
      "⚠️ No Vitest report was produced. See the job log.",
      "",
    ].join("\n");
  }

  const ordered = [...summaries].sort(
    (left, right) =>
      (suiteDetails[left.id]?.order ?? 1_000) -
      (suiteDetails[right.id]?.order ?? 1_000),
  );
  const totals = ordered.reduce(
    (current, summary) => ({
      files: {
        failed: current.files.failed + summary.files.failed,
        passed: current.files.passed + summary.files.passed,
        total: current.files.total + summary.files.total,
      },
      skipped: current.skipped + summary.skipped,
      tests: {
        failed: current.tests.failed + summary.tests.failed,
        passed: current.tests.passed + summary.tests.passed,
        total: current.tests.total + summary.tests.total,
      },
    }),
    {
      files: { failed: 0, passed: 0, total: 0 },
      skipped: 0,
      tests: { failed: 0, passed: 0, total: 0 },
    },
  );
  const lines = [
    "# Test report",
    "",
    "| Suite | Result | Test files passed | Tests passed | Skipped |",
    "| --- | --- | ---: | ---: | ---: |",
    ...ordered.map(
      (summary) =>
        `| ${summary.name} | ${result(summary)} | ${passed(summary.files)} | ${passed(summary.tests)} | ${summary.skipped || "—"} |`,
    ),
    `| **Total** | **${totals.files.failed > 0 || totals.tests.failed > 0 ? "❌ Failed" : "✅ Passed"}** | **${passed(totals.files)}** | **${passed(totals.tests)}** | **${totals.skipped || "—"}** |`,
    "",
  ];

  if (ordered.some((summary) => summary.id === "desktop" && summary.skipped)) {
    lines.push(
      "> The Desktop suite does not run one macOS-only runner test on Ubuntu. The Native app job runs this test on macOS.",
      "",
    );
  }

  return lines.join("\n");
}
