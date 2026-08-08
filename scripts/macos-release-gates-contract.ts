const SAMPLE_COUNT = 5;

export const MEGABYTE = 1_000_000;
const GIBIBYTE = 1_024 * 1_024 * 1_024;
export const REFERENCE_MEMORY_BYTES = 24 * GIBIBYTE;
export const MACOS_RELEASE_GATES_SCHEMA_VERSION = "touchgrass.macos-release-gates.v1" as const;
export const REFRESH_FIXTURE_VERSION = "touchgrass.refresh-fixture.v1" as const;
export const REFRESH_FIXTURE_SHA256 =
  "7103ab94462c700e5acdde3d4ac929c1f33456fad90dbafe881dedf4576c79d1" as const;
export const REFRESH_FIXTURE_BYTES = 83_348 as const;

export const MACOS_RELEASE_GATE_LIMITS = {
  startup_ms: { median: 1_000, worst: 2_000 },
  panel_paint_ms: { median: 100, worst: 200 },
  idle_cpu_percent: { median: 0.5, worst: 1 },
  settled_rss_bytes: {
    median: 200 * MEGABYTE,
    worst: 250 * MEGABYTE,
  },
  app_bytes: 40 * MEGABYTE,
  dmg_bytes: 25 * MEGABYTE,
  refresh: {
    panel_paint_ms: { worst: 200 },
    average_cpu_percent: { worst: 25 },
    peak_rss_bytes: { worst: 250 * MEGABYTE },
    recovery_to_idle_ms: { worst: 5_000 },
  },
} as const;

type GateStatus = "PASS" | "FAIL";
type FiveSamples = readonly [number, number, number, number, number];

type CandidateBinding = {
  version: string;
  commit: string;
  artifact_sha256: string;
  app_bytes: number;
  dmg_bytes: number;
};

type HardwareBinding = {
  model: string;
  chip: string;
  memory_bytes: number;
};

type EnvironmentBinding = {
  hardware: HardwareBinding;
  power: {
    source: "AC";
    low_power_mode: false;
  };
  macos_version: string;
};

type FixtureBinding = {
  version: typeof REFRESH_FIXTURE_VERSION;
  sha256: string;
  bytes: number;
};

type RefreshSamples = {
  panel_paint_ms: FiveSamples;
  average_cpu_percent: FiveSamples;
  peak_rss_bytes: FiveSamples;
  recovery_to_idle_ms: FiveSamples;
};

type MeasurementSamples = {
  startup_ms: FiveSamples;
  panel_paint_ms: FiveSamples;
  idle_cpu_percent: FiveSamples;
  settled_rss_bytes: FiveSamples;
  refresh: RefreshSamples;
};

type AutomatedPreflight = {
  positioning_clamping: GateStatus;
  toggling: GateStatus;
  escape: GateStatus;
  outside_click: GateStatus;
  rapid_interaction: GateStatus;
  persisted_launch_at_login: GateStatus;
  current_space: GateStatus;
  macos_15_floor: GateStatus;
  latest_stable: GateStatus;
};

export type MacosReleaseGatesInput = {
  schema_version: typeof MACOS_RELEASE_GATES_SCHEMA_VERSION;
  candidate: CandidateBinding;
  environment: EnvironmentBinding;
  fixture: FixtureBinding;
  samples: MeasurementSamples;
  automated_preflight: AutomatedPreflight;
};

type MetricReport = {
  raw: FiveSamples;
  median: number;
  worst: number;
  status: GateStatus;
};

type ArtifactReport = {
  bytes: number;
  limit_bytes: number;
  status: GateStatus;
};

export type MacosReleaseGatesReport = {
  schema_version: typeof MACOS_RELEASE_GATES_SCHEMA_VERSION;
  status: GateStatus;
  candidate: CandidateBinding;
  environment: EnvironmentBinding;
  environment_status: GateStatus;
  fixture: FixtureBinding;
  limits: typeof MACOS_RELEASE_GATE_LIMITS;
  metrics: {
    startup_ms: MetricReport;
    panel_paint_ms: MetricReport;
    idle_cpu_percent: MetricReport;
    settled_rss_bytes: MetricReport;
    refresh: {
      panel_paint_ms: MetricReport;
      average_cpu_percent: MetricReport;
      peak_rss_bytes: MetricReport;
      recovery_to_idle_ms: MetricReport;
    };
  };
  artifacts: {
    app: ArtifactReport;
    dmg: ArtifactReport;
  };
  automated_preflight: AutomatedPreflight;
};

type JsonObject = Record<string, unknown>;

const ROOT_FIELDS = [
  "schema_version",
  "candidate",
  "environment",
  "fixture",
  "samples",
  "automated_preflight",
] as const;
const CANDIDATE_FIELDS = [
  "version",
  "commit",
  "artifact_sha256",
  "app_bytes",
  "dmg_bytes",
] as const;
const ENVIRONMENT_FIELDS = ["hardware", "power", "macos_version"] as const;
const HARDWARE_FIELDS = ["model", "chip", "memory_bytes"] as const;
const POWER_FIELDS = ["source", "low_power_mode"] as const;
const FIXTURE_FIELDS = ["version", "sha256", "bytes"] as const;
const SAMPLE_FIELDS = [
  "startup_ms",
  "panel_paint_ms",
  "idle_cpu_percent",
  "settled_rss_bytes",
  "refresh",
] as const;
const REFRESH_SAMPLE_FIELDS = [
  "panel_paint_ms",
  "average_cpu_percent",
  "peak_rss_bytes",
  "recovery_to_idle_ms",
] as const;
const PREFLIGHT_FIELDS = [
  "positioning_clamping",
  "toggling",
  "escape",
  "outside_click",
  "rapid_interaction",
  "persisted_launch_at_login",
  "current_space",
  "macos_15_floor",
  "latest_stable",
] as const;

function invalid(path: string): never {
  throw new Error(`Invalid macOS release gates input at ${path}.`);
}

function exactObject(value: unknown, path: string, fields: readonly string[]): JsonObject {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return invalid(path);
  }
  const prototype = Object.getPrototypeOf(value);
  if (prototype !== Object.prototype && prototype !== null) return invalid(path);

  const allowed = new Set(fields);
  const keys = Reflect.ownKeys(value);
  if (
    keys.length !== fields.length ||
    keys.some((key) => typeof key !== "string" || !allowed.has(key)) ||
    fields.some((field) => !Object.hasOwn(value, field))
  ) {
    return invalid(path);
  }
  return value as JsonObject;
}

function exactString(value: unknown, path: string, pattern: RegExp): string {
  if (typeof value !== "string" || !pattern.test(value)) return invalid(path);
  return value;
}

function exactLiteral<Value extends string | boolean>(
  value: unknown,
  expected: Value,
  path: string,
): Value {
  if (value !== expected) return invalid(path);
  return expected;
}

function positiveSafeInteger(value: unknown, path: string): number {
  if (!Number.isSafeInteger(value) || (value as number) <= 0) return invalid(path);
  return value as number;
}

function fiveSamples(value: unknown, path: string): FiveSamples {
  const expectedKeys = ["0", "1", "2", "3", "4", "length"];
  if (
    !Array.isArray(value) ||
    Object.getPrototypeOf(value) !== Array.prototype ||
    value.length !== SAMPLE_COUNT ||
    Reflect.ownKeys(value).length !== expectedKeys.length ||
    expectedKeys.some((key) => !Object.hasOwn(value, key)) ||
    value.some((sample) => typeof sample !== "number" || !Number.isFinite(sample) || sample < 0)
  ) {
    return invalid(path);
  }
  return [...value] as [number, number, number, number, number];
}

function gateStatus(value: unknown, path: string): GateStatus {
  if (value !== "PASS" && value !== "FAIL") return invalid(path);
  return value;
}

function parseCandidate(value: unknown): CandidateBinding {
  const candidate = exactObject(value, "candidate", CANDIDATE_FIELDS);
  return {
    version: exactString(
      candidate.version,
      "candidate.version",
      /^(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)$/,
    ),
    commit: exactString(candidate.commit, "candidate.commit", /^[0-9a-f]{40}$/),
    artifact_sha256: exactString(
      candidate.artifact_sha256,
      "candidate.artifact_sha256",
      /^[0-9a-f]{64}$/,
    ),
    app_bytes: positiveSafeInteger(candidate.app_bytes, "candidate.app_bytes"),
    dmg_bytes: positiveSafeInteger(candidate.dmg_bytes, "candidate.dmg_bytes"),
  };
}

const SANITIZED_HARDWARE_TEXT = /^[A-Za-z0-9][A-Za-z0-9 .,_()+-]{0,79}$/;

function parseEnvironment(value: unknown): EnvironmentBinding {
  const environment = exactObject(value, "environment", ENVIRONMENT_FIELDS);
  const hardware = exactObject(environment.hardware, "environment.hardware", HARDWARE_FIELDS);
  const power = exactObject(environment.power, "environment.power", POWER_FIELDS);
  return {
    hardware: {
      model: exactString(hardware.model, "environment.hardware.model", SANITIZED_HARDWARE_TEXT),
      chip: exactString(hardware.chip, "environment.hardware.chip", SANITIZED_HARDWARE_TEXT),
      memory_bytes: positiveSafeInteger(hardware.memory_bytes, "environment.hardware.memory_bytes"),
    },
    power: {
      source: exactLiteral(power.source, "AC", "environment.power.source"),
      low_power_mode: exactLiteral(power.low_power_mode, false, "environment.power.low_power_mode"),
    },
    macos_version: exactString(
      environment.macos_version,
      "environment.macos_version",
      /^(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)(?:\.(?:0|[1-9]\d*))?$/,
    ),
  };
}

function parseFixture(value: unknown): FixtureBinding {
  const fixture = exactObject(value, "fixture", FIXTURE_FIELDS);
  if (fixture.sha256 !== REFRESH_FIXTURE_SHA256) {
    return invalid("fixture.sha256");
  }
  if (fixture.bytes !== REFRESH_FIXTURE_BYTES) {
    return invalid("fixture.bytes");
  }
  return {
    version: exactLiteral(fixture.version, REFRESH_FIXTURE_VERSION, "fixture.version"),
    sha256: REFRESH_FIXTURE_SHA256,
    bytes: REFRESH_FIXTURE_BYTES,
  };
}

function parseSamples(value: unknown): MeasurementSamples {
  const samples = exactObject(value, "samples", SAMPLE_FIELDS);
  const refresh = exactObject(samples.refresh, "samples.refresh", REFRESH_SAMPLE_FIELDS);
  return {
    startup_ms: fiveSamples(samples.startup_ms, "samples.startup_ms"),
    panel_paint_ms: fiveSamples(samples.panel_paint_ms, "samples.panel_paint_ms"),
    idle_cpu_percent: fiveSamples(samples.idle_cpu_percent, "samples.idle_cpu_percent"),
    settled_rss_bytes: fiveSamples(samples.settled_rss_bytes, "samples.settled_rss_bytes"),
    refresh: {
      panel_paint_ms: fiveSamples(refresh.panel_paint_ms, "samples.refresh.panel_paint_ms"),
      average_cpu_percent: fiveSamples(
        refresh.average_cpu_percent,
        "samples.refresh.average_cpu_percent",
      ),
      peak_rss_bytes: fiveSamples(refresh.peak_rss_bytes, "samples.refresh.peak_rss_bytes"),
      recovery_to_idle_ms: fiveSamples(
        refresh.recovery_to_idle_ms,
        "samples.refresh.recovery_to_idle_ms",
      ),
    },
  };
}

function parsePreflight(value: unknown): AutomatedPreflight {
  const preflight = exactObject(value, "automated_preflight", PREFLIGHT_FIELDS);
  return {
    positioning_clamping: gateStatus(
      preflight.positioning_clamping,
      "automated_preflight.positioning_clamping",
    ),
    toggling: gateStatus(preflight.toggling, "automated_preflight.toggling"),
    escape: gateStatus(preflight.escape, "automated_preflight.escape"),
    outside_click: gateStatus(preflight.outside_click, "automated_preflight.outside_click"),
    rapid_interaction: gateStatus(
      preflight.rapid_interaction,
      "automated_preflight.rapid_interaction",
    ),
    persisted_launch_at_login: gateStatus(
      preflight.persisted_launch_at_login,
      "automated_preflight.persisted_launch_at_login",
    ),
    current_space: gateStatus(preflight.current_space, "automated_preflight.current_space"),
    macos_15_floor: gateStatus(preflight.macos_15_floor, "automated_preflight.macos_15_floor"),
    latest_stable: gateStatus(preflight.latest_stable, "automated_preflight.latest_stable"),
  };
}

function parseInput(input: unknown): MacosReleaseGatesInput {
  const root = exactObject(input, "root", ROOT_FIELDS);
  return {
    schema_version: exactLiteral(
      root.schema_version,
      MACOS_RELEASE_GATES_SCHEMA_VERSION,
      "schema_version",
    ),
    candidate: parseCandidate(root.candidate),
    environment: parseEnvironment(root.environment),
    fixture: parseFixture(root.fixture),
    samples: parseSamples(root.samples),
    automated_preflight: parsePreflight(root.automated_preflight),
  };
}

function metricReport(raw: FiveSamples, limits: { median?: number; worst: number }): MetricReport {
  const ordered = [...raw].sort((left, right) => left - right);
  const median = ordered[2];
  const worst = ordered[ordered.length - 1];
  const medianPasses = limits.median === undefined || median <= limits.median;
  return {
    raw,
    median,
    worst,
    status: medianPasses && worst <= limits.worst ? "PASS" : "FAIL",
  };
}

function artifactReport(bytes: number, limitBytes: number): ArtifactReport {
  return {
    bytes,
    limit_bytes: limitBytes,
    status: bytes <= limitBytes ? "PASS" : "FAIL",
  };
}

function environmentStatus(environment: EnvironmentBinding): GateStatus {
  const macosMajor = Number(environment.macos_version.split(".", 1)[0]);
  return environment.hardware.chip === "Apple M4 Pro" &&
    environment.hardware.memory_bytes === REFERENCE_MEMORY_BYTES &&
    macosMajor >= 15
    ? "PASS"
    : "FAIL";
}

function allPass(statuses: readonly GateStatus[]): GateStatus {
  return statuses.every((status) => status === "PASS") ? "PASS" : "FAIL";
}

export function evaluateMacosReleaseGates(input: unknown): MacosReleaseGatesReport {
  const parsed = parseInput(input);
  const metrics = {
    startup_ms: metricReport(parsed.samples.startup_ms, MACOS_RELEASE_GATE_LIMITS.startup_ms),
    panel_paint_ms: metricReport(
      parsed.samples.panel_paint_ms,
      MACOS_RELEASE_GATE_LIMITS.panel_paint_ms,
    ),
    idle_cpu_percent: metricReport(
      parsed.samples.idle_cpu_percent,
      MACOS_RELEASE_GATE_LIMITS.idle_cpu_percent,
    ),
    settled_rss_bytes: metricReport(
      parsed.samples.settled_rss_bytes,
      MACOS_RELEASE_GATE_LIMITS.settled_rss_bytes,
    ),
    refresh: {
      panel_paint_ms: metricReport(
        parsed.samples.refresh.panel_paint_ms,
        MACOS_RELEASE_GATE_LIMITS.refresh.panel_paint_ms,
      ),
      average_cpu_percent: metricReport(
        parsed.samples.refresh.average_cpu_percent,
        MACOS_RELEASE_GATE_LIMITS.refresh.average_cpu_percent,
      ),
      peak_rss_bytes: metricReport(
        parsed.samples.refresh.peak_rss_bytes,
        MACOS_RELEASE_GATE_LIMITS.refresh.peak_rss_bytes,
      ),
      recovery_to_idle_ms: metricReport(
        parsed.samples.refresh.recovery_to_idle_ms,
        MACOS_RELEASE_GATE_LIMITS.refresh.recovery_to_idle_ms,
      ),
    },
  };
  const artifacts = {
    app: artifactReport(parsed.candidate.app_bytes, MACOS_RELEASE_GATE_LIMITS.app_bytes),
    dmg: artifactReport(parsed.candidate.dmg_bytes, MACOS_RELEASE_GATE_LIMITS.dmg_bytes),
  };
  const checkedEnvironment = environmentStatus(parsed.environment);
  const metricStatuses = [
    metrics.startup_ms.status,
    metrics.panel_paint_ms.status,
    metrics.idle_cpu_percent.status,
    metrics.settled_rss_bytes.status,
    metrics.refresh.panel_paint_ms.status,
    metrics.refresh.average_cpu_percent.status,
    metrics.refresh.peak_rss_bytes.status,
    metrics.refresh.recovery_to_idle_ms.status,
  ];

  return {
    schema_version: parsed.schema_version,
    status: allPass([
      checkedEnvironment,
      artifacts.app.status,
      artifacts.dmg.status,
      ...metricStatuses,
      ...Object.values(parsed.automated_preflight),
    ]),
    candidate: parsed.candidate,
    environment: parsed.environment,
    environment_status: checkedEnvironment,
    fixture: parsed.fixture,
    limits: MACOS_RELEASE_GATE_LIMITS,
    metrics,
    artifacts,
    automated_preflight: parsed.automated_preflight,
  };
}
