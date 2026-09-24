import { expect, test } from "vitest";
import fixtures from "../fixtures/diagnostic-reports-v1.json";
import { diagnosticReportSchema, supportReportSchema, usageScanContextSchema } from "./diagnostics";

const report = fixtures.find((item) => item.failure.context.reason === "scan_incomplete")!;
const scan = report.failure.context.scan!;

test("old failure reports and new bounded scan reports remain valid", () => {
  for (const fixture of fixtures)
    expect(diagnosticReportSchema.safeParse(fixture).success).toBe(true);
  expect(
    supportReportSchema.safeParse({
      schemaVersion: 1,
      appVersion: "0.0.53",
      capturedAt: 1,
      claudeScan: scan,
    }).success,
  ).toBe(true);
});

test("scan evidence rejects private fields, unbounded days, and impossible counts", () => {
  for (const value of [
    { ...scan, path: "/private/project" },
    { ...scan, files: { ...scan.files, paths: ["/private/project"] } },
    { ...scan, days: Array.from({ length: 31 }, () => scan.days[0]) },
    { ...scan, days: [{ ...scan.days[0], pricedTokens: 1000001 }] },
    { ...scan, days: [{ ...scan.days[0], costMicros: -1 }] },
    { ...scan, days: [scan.days[0], scan.days[0]] },
  ])
    expect(usageScanContextSchema.safeParse(value).success).toBe(false);
});
