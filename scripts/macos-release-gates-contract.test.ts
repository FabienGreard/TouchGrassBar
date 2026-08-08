import { describe, expect, test } from "vitest";

import {
  MACOS_RELEASE_GATES_SCHEMA_VERSION,
  MEGABYTE,
  REFERENCE_MEMORY_BYTES,
  REFRESH_FIXTURE_BYTES,
  REFRESH_FIXTURE_SHA256,
  REFRESH_FIXTURE_VERSION,
  evaluateMacosReleaseGates,
} from "./macos-release-gates-contract";

const pass = "PASS" as const;

function validInput() {
  return {
    schema_version: MACOS_RELEASE_GATES_SCHEMA_VERSION,
    candidate: {
      version: "1.2.3",
      commit: "a".repeat(40),
      artifact_sha256: "b".repeat(64),
      app_bytes: 40 * MEGABYTE,
      dmg_bytes: 25 * MEGABYTE,
    },
    environment: {
      hardware: {
        model: "Mac16,8",
        chip: "Apple M4 Pro",
        memory_bytes: REFERENCE_MEMORY_BYTES,
      },
      power: {
        source: "AC",
        low_power_mode: false,
      },
      macos_version: "15.7.1",
    },
    fixture: {
      version: REFRESH_FIXTURE_VERSION,
      sha256: String(REFRESH_FIXTURE_SHA256),
      bytes: REFRESH_FIXTURE_BYTES,
    },
    samples: {
      startup_ms: [1_000, 800, 1_000, 2_000, 1_000],
      panel_paint_ms: [100, 50, 100, 200, 100],
      idle_cpu_percent: [0.5, 0.2, 0.5, 1, 0.5],
      settled_rss_bytes: [
        200 * MEGABYTE,
        100 * MEGABYTE,
        200 * MEGABYTE,
        250 * MEGABYTE,
        200 * MEGABYTE,
      ],
      refresh: {
        panel_paint_ms: [100, 125, 150, 175, 200],
        average_cpu_percent: [5, 10, 15, 20, 25],
        peak_rss_bytes: [
          100 * MEGABYTE,
          150 * MEGABYTE,
          200 * MEGABYTE,
          225 * MEGABYTE,
          250 * MEGABYTE,
        ],
        recovery_to_idle_ms: [1_000, 2_000, 3_000, 4_000, 5_000],
      },
    },
    automated_preflight: {
      positioning_clamping: pass,
      toggling: pass,
      escape: pass,
      outside_click: pass,
      rapid_interaction: pass,
      persisted_launch_at_login: pass,
      current_space: pass,
      macos_15_floor: pass,
      latest_stable: pass,
    },
  };
}

function sparseSamples() {
  const samples = [1, 2, 3, 4, 5];
  Reflect.deleteProperty(samples, "4");
  return samples;
}

describe("macOS release gates contract", () => {
  test("recomputes raw, median, and worst values at every approved limit", () => {
    const report = evaluateMacosReleaseGates(validInput());

    expect(report.status).toBe("PASS");
    expect(report.metrics.startup_ms).toEqual({
      raw: [1_000, 800, 1_000, 2_000, 1_000],
      median: 1_000,
      worst: 2_000,
      status: "PASS",
    });
    expect(report.metrics.panel_paint_ms).toMatchObject({
      median: 100,
      worst: 200,
      status: "PASS",
    });
    expect(report.metrics.idle_cpu_percent).toMatchObject({
      median: 0.5,
      worst: 1,
      status: "PASS",
    });
    expect(report.metrics.settled_rss_bytes).toMatchObject({
      median: 200 * MEGABYTE,
      worst: 250 * MEGABYTE,
      status: "PASS",
    });
    expect(report.metrics.refresh).toMatchObject({
      panel_paint_ms: { median: 150, worst: 200, status: "PASS" },
      average_cpu_percent: { median: 15, worst: 25, status: "PASS" },
      peak_rss_bytes: {
        median: 200 * MEGABYTE,
        worst: 250 * MEGABYTE,
        status: "PASS",
      },
      recovery_to_idle_ms: {
        median: 3_000,
        worst: 5_000,
        status: "PASS",
      },
    });
    expect(report.artifacts).toEqual({
      app: { bytes: 40 * MEGABYTE, limit_bytes: 40 * MEGABYTE, status: "PASS" },
      dmg: { bytes: 25 * MEGABYTE, limit_bytes: 25 * MEGABYTE, status: "PASS" },
    });
  });

  test("returns FAIL for any metric, size, environment, or preflight violation", () => {
    const metric = validInput();
    metric.samples.startup_ms = [1_001, 1_001, 1_001, 1_500, 2_000];
    expect(evaluateMacosReleaseGates(metric)).toMatchObject({
      status: "FAIL",
      metrics: { startup_ms: { status: "FAIL" } },
    });

    const size = validInput();
    size.candidate.dmg_bytes += 1;
    expect(evaluateMacosReleaseGates(size)).toMatchObject({
      status: "FAIL",
      artifacts: { dmg: { status: "FAIL" } },
    });

    const binaryMegabyteSize = validInput();
    binaryMegabyteSize.candidate.app_bytes = 40 * 1_024 * 1_024;
    expect(evaluateMacosReleaseGates(binaryMegabyteSize)).toMatchObject({
      status: "FAIL",
      artifacts: { app: { limit_bytes: 40_000_000, status: "FAIL" } },
    });

    const hardware = validInput();
    hardware.environment.hardware.chip = "Apple M4";
    expect(evaluateMacosReleaseGates(hardware)).toMatchObject({
      environment_status: "FAIL",
      status: "FAIL",
    });

    const preflight = validInput();
    (preflight.automated_preflight as Record<string, string>).current_space = "FAIL";
    expect(evaluateMacosReleaseGates(preflight)).toMatchObject({ status: "FAIL" });
  });

  test.each([
    ["too few", [1, 2, 3, 4]],
    ["too many", [1, 2, 3, 4, 5, 6]],
    ["sparse", sparseSamples()],
    ["unknown array field", Object.assign([1, 2, 3, 4, 5], { extra: true })],
    ["negative", [1, 2, 3, 4, -1]],
    ["NaN", [1, 2, 3, 4, Number.NaN]],
    ["infinity", [1, 2, 3, 4, Number.POSITIVE_INFINITY]],
  ])("rejects %s samples", (_name, samples) => {
    const input = validInput();
    (input.samples as Record<string, unknown>).startup_ms = samples;

    expect(() => evaluateMacosReleaseGates(input)).toThrow("Invalid macOS release gates input");
  });

  test("requires exact objects at every contract boundary", () => {
    const root = { ...validInput(), unknown: true };
    expect(() => evaluateMacosReleaseGates(root)).toThrow("Invalid macOS release gates input");

    const nested = validInput() as ReturnType<typeof validInput> & {
      candidate: ReturnType<typeof validInput>["candidate"] & { local_path?: string };
    };
    nested.candidate.local_path = "/private/release";
    expect(() => evaluateMacosReleaseGates(nested)).toThrow("Invalid macOS release gates input");

    const missing = validInput() as Record<string, unknown>;
    delete (missing.fixture as Record<string, unknown>).sha256;
    expect(() => evaluateMacosReleaseGates(missing)).toThrow("Invalid macOS release gates input");
  });

  test.each([
    ["version", (input: ReturnType<typeof validInput>) => (input.candidate.version = "v1.2.3")],
    ["commit", (input: ReturnType<typeof validInput>) => (input.candidate.commit = "A".repeat(40))],
    [
      "artifact digest",
      (input: ReturnType<typeof validInput>) => (input.candidate.artifact_sha256 = "not-a-digest"),
    ],
    [
      "model",
      (input: ReturnType<typeof validInput>) =>
        (input.environment.hardware.model = "/private/model"),
    ],
    [
      "macOS version",
      (input: ReturnType<typeof validInput>) => (input.environment.macos_version = "latest"),
    ],
    [
      "fixture version",
      (input: ReturnType<typeof validInput>) =>
        ((input.fixture as { version: string }).version = "touchgrass.refresh-fixture.v0"),
    ],
    [
      "fixture digest",
      (input: ReturnType<typeof validInput>) => (input.fixture.sha256 = "C".repeat(64)),
    ],
    [
      "well-formed but stale fixture digest",
      (input: ReturnType<typeof validInput>) => (input.fixture.sha256 = "c".repeat(64)),
    ],
    ["fixture byte count", (input: ReturnType<typeof validInput>) => (input.fixture.bytes += 1)],
  ])("rejects an invalid %s binding", (_name, mutate) => {
    const input = validInput();
    mutate(input);

    expect(() => evaluateMacosReleaseGates(input)).toThrow("Invalid macOS release gates input");
  });

  test("rejects non-closed or stale preflight states", () => {
    for (const state of ["STALE", "NOT_RUN", "pass"] as const) {
      const input = validInput();
      (input.automated_preflight as Record<string, string>).current_space = state;

      expect(() => evaluateMacosReleaseGates(input)).toThrow("Invalid macOS release gates input");
    }
  });
});
