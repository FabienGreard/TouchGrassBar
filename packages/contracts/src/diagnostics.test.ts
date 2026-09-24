import { expect, test } from "vitest";
import fixtures from "../fixtures/diagnostic-reports-v1.json";
import { diagnosticReportSchema, usageScanContextSchema } from "./diagnostics";

const report = fixtures.find((item) => item.failure.context.reason === "scan_incomplete")!;
const scan = report.failure.context.scan!;

test("old failure reports and new bounded scan reports remain valid", () => {
  for (const fixture of fixtures)
    expect(diagnosticReportSchema.safeParse(fixture).success).toBe(true);
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

const pricingReport = fixtures.find((item) => item.failure.area === "pricing")!;
const modelReport = (provider: string, model: unknown, reason = "unknown_model") => ({
  ...pricingReport,
  failure: {
    ...pricingReport.failure,
    code: "pricing_calculation_failed",
    provider,
    context: { ...pricingReport.failure.context, reason, model },
  },
});

test("unknown model names are bounded and cannot contain source text or paths", () => {
  for (const [provider, model] of [
    ["codex", "gpt-99-one"],
    ["codex", "o9"],
    ["claude", "claude-future-99"],
  ]) {
    expect(diagnosticReportSchema.safeParse(modelReport(provider!, model)).success).toBe(true);
  }
  for (const model of [
    "/private/model",
    "gpt-99 secret",
    "gpt-99\nsecret",
    "gpt-99@host",
    "gpt-99/secret",
    "gpt-" + "x".repeat(97),
    null,
  ]) {
    expect(diagnosticReportSchema.safeParse(modelReport("codex", model)).success).toBe(false);
  }
  expect(diagnosticReportSchema.safeParse(modelReport("claude", "gpt-99-one")).success).toBe(false);
  expect(
    diagnosticReportSchema.safeParse(modelReport("codex", "gpt-99-one", "missing_effective_price"))
      .success,
  ).toBe(false);
});

test("rejection evidence permits only fixed codes in the relevant Claude failure", () => {
  const original = fixtures.find((r) => "rejection" in r.failure.context)!;
  const evidence = { field: "model", problem: "missing", tokenCounters: "nonzero" };
  const check = (rejection: unknown, provider = "claude", reason = "invalid_message_metadata") =>
    diagnosticReportSchema.safeParse({
      ...original,
      failure: {
        ...original.failure,
        provider,
        context: { ...original.failure.context, reason, rejection },
      },
    }).success;
  expect(check(evidence)).toBe(true);
  for (const value of [
    null,
    { ...evidence, field: "/private/path" },
    { ...evidence, problem: "raw error" },
    { ...evidence, tokenCounters: 123 },
    { ...evidence, content: "private" },
  ])
    expect(check(value)).toBe(false);
  expect(check(evidence, "codex")).toBe(false);
  expect(check(evidence, "claude", "scan_incomplete")).toBe(false);
});
