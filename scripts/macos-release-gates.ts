#!/usr/bin/env bun

import { execFile, execFileSync, spawn } from "node:child_process";
import { createHash } from "node:crypto";
import {
  existsSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";
import { createInterface } from "node:readline";

import {
  MACOS_RELEASE_GATES_SCHEMA_VERSION,
  REFERENCE_MEMORY_BYTES,
  evaluateMacosReleaseGates,
  type MacosReleaseGatesInput,
} from "./macos-release-gates-contract";
import {
  createMacosRefreshFixtureBinding,
  generateMacosRefreshFixtureBytes,
} from "./macos-refresh-fixture";
import {
  parseMacosProcessTable,
  sumMacosProcessTree,
  type MacosProcessTreeTotals,
} from "./macos-process-tree";

const executeFile = promisify(execFile);
const workspaceRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const repository = "FabienGreard/TouchGrassBar";
const sampleCount = 5;
const idleSettleMilliseconds = 60_000;
const idleWindowMilliseconds = 10_000;
const processSampleMilliseconds = 1_000;
const commandTimeoutMilliseconds = 15_000;

type GateStatus = "PASS" | "FAIL";
type ReleaseGateArguments = {
  dmg: string;
  output: string;
  trust: string;
};
type CandidateBinding = MacosReleaseGatesInput["candidate"];
type LocalPreflight = {
  currentSpace: boolean;
  escape: boolean;
  outsideClick: boolean;
  persistedLaunchAtLogin: boolean;
  positioningClamping: boolean;
  rapidInteraction: boolean;
  toggling: boolean;
};
type CiEvidence = {
  conclusion: string;
  headSha: string;
  jobs: Array<{ conclusion: string; name: string }>;
};
const releaseGateEvents = [
  "driver_failed",
  "hide_failed",
  "hidden",
  "invalid_command",
  "launch_at_login_failed",
  "launch_at_login_pass",
  "menu_bar_ready",
  "outside_click_failed",
  "outside_click_pass",
  "rapid_interaction_failed",
  "rapid_interaction_pass",
  "refresh_complete",
  "refresh_failed",
  "refresh_started",
  "show_accepted",
  "show_failed",
  "toggled_hidden",
] as const;
type ReleaseGateEvent = (typeof releaseGateEvents)[number];
const releaseGateEventSet = new Set<string>(releaseGateEvents);
const sha256Pattern = /^[0-9a-f]{64}$/;
const commitPattern = /^[0-9a-f]{40}$/;
const versionPattern = /^(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)$/;
const macosVersionPattern = /^(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)(?:\.(?:0|[1-9][0-9]*))?$/;

function usage(): never {
  throw new Error(
    "Usage: bun run macos:release-gates -- --dmg <file> --trust <file> --output <file>",
  );
}

export function parseMacosReleaseGateArguments(
  argumentsList: readonly string[],
): ReleaseGateArguments {
  if (argumentsList.length !== 6) return usage();
  const parsed = new Map<string, string>();
  for (let index = 0; index < argumentsList.length; index += 2) {
    const option = argumentsList[index];
    const value = argumentsList[index + 1];
    if (
      !option ||
      !value ||
      !["--dmg", "--output", "--trust"].includes(option) ||
      parsed.has(option) ||
      value.trim() === ""
    ) {
      return usage();
    }
    parsed.set(option, value);
  }
  const dmg = parsed.get("--dmg");
  const output = parsed.get("--output");
  const trust = parsed.get("--trust");
  if (!dmg || !output || !trust) return usage();
  return { dmg, output, trust };
}

function parseJsonObject(value: unknown): Record<string, unknown> | null {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

function trustedPass(value: unknown, fields: readonly string[]): boolean {
  const record = parseJsonObject(value);
  return record !== null && fields.every((field) => record[field] === "PASS");
}

type CandidateBindingInput = {
  appBytes: number;
  appVersion: string;
  dmgBytes: number;
  dmgName: string;
  dmgSha256: string;
  receipt: unknown;
};

function receiptMismatch(): never {
  throw new Error("Release trust receipt does not bind the measured candidate.");
}

export function bindReleaseCandidate({
  appBytes,
  appVersion,
  dmgBytes,
  dmgName,
  dmgSha256,
  receipt,
}: CandidateBindingInput): {
  candidate: CandidateBinding;
  mainCiRunId: string;
} {
  const root = parseJsonObject(receipt);
  const candidate = parseJsonObject(root?.candidate);
  const distribution = parseJsonObject(root?.distribution_trust);
  const redaction = parseJsonObject(root?.redaction);
  const artifacts = root?.artifacts;
  if (
    root?.schema_version !== "touchgrass.release-trust.v1" ||
    !candidate ||
    !distribution ||
    !redaction ||
    !Array.isArray(artifacts) ||
    !versionPattern.test(String(candidate.version ?? "")) ||
    candidate.tag !== `v${String(candidate.version)}` ||
    !commitPattern.test(String(candidate.commit ?? "")) ||
    !/^[1-9][0-9]*$/.test(String(candidate.main_ci_run_id ?? "")) ||
    candidate.version !== appVersion ||
    !Number.isSafeInteger(appBytes) ||
    appBytes <= 0 ||
    !Number.isSafeInteger(dmgBytes) ||
    dmgBytes <= 0 ||
    !sha256Pattern.test(dmgSha256) ||
    !trustedPass(distribution.app, [
      "gatekeeper",
      "hardened_runtime",
      "notarization",
      "stapling",
      "timestamp",
    ]) ||
    parseJsonObject(distribution.app)?.architecture !== "arm64" ||
    !trustedPass(distribution.dmg, ["gatekeeper", "notarization", "stapling"]) ||
    distribution.updater_signature !== "PASS" ||
    redaction.credential_material !== "ABSENT" ||
    redaction.private_paths !== "ABSENT" ||
    redaction.raw_provider_responses !== "ABSENT" ||
    redaction.runner_paths !== "ABSENT"
  ) {
    return receiptMismatch();
  }

  const matchingArtifacts = artifacts.filter((value) => {
    const artifact = parseJsonObject(value);
    return artifact?.name === dmgName;
  });
  const artifact = parseJsonObject(matchingArtifacts[0]);
  if (
    matchingArtifacts.length !== 1 ||
    artifact?.bytes !== dmgBytes ||
    artifact.sha256 !== dmgSha256
  ) {
    return receiptMismatch();
  }

  return {
    candidate: {
      version: String(candidate.version),
      commit: String(candidate.commit),
      artifact_sha256: dmgSha256,
      app_bytes: appBytes,
      dmg_bytes: dmgBytes,
    },
    mainCiRunId: String(candidate.main_ci_run_id),
  };
}

export function parseReleaseGateDriverEvent(line: string): ReleaseGateEvent | null {
  const match = /^touchgrassbar_release_gate event=([^ ]+)$/.exec(line.trim());
  return match?.[1] && releaseGateEventSet.has(match[1]) ? (match[1] as ReleaseGateEvent) : null;
}

export function parsePanelPaintMetric(line: string): number | null {
  const match =
    /^touchgrassbar_metric panel_paint_source=synthetic panel_paint_ms=((?:0|[1-9][0-9]*)(?:\.[0-9]+)?)$/.exec(
      line.trim(),
    );
  if (!match?.[1]) return null;
  const value = Number(match[1]);
  return Number.isFinite(value) ? value : null;
}

function status(value: boolean): GateStatus {
  return value ? "PASS" : "FAIL";
}

export function buildAutomatedPreflight(
  local: LocalPreflight,
  ci: CiEvidence,
  expectedCommit: string,
): MacosReleaseGatesInput["automated_preflight"] {
  const ciIdentityPasses = ci.conclusion === "success" && ci.headSha === expectedCommit;
  const jobPasses = (marker: string) =>
    ciIdentityPasses &&
    ci.jobs.some((job) => job.conclusion === "success" && job.name.includes(marker));
  return {
    positioning_clamping: status(local.positioningClamping),
    toggling: status(local.toggling),
    escape: status(local.escape),
    outside_click: status(local.outsideClick),
    rapid_interaction: status(local.rapidInteraction),
    persisted_launch_at_login: status(local.persistedLaunchAtLogin),
    current_space: status(local.currentSpace),
    macos_15_floor: status(jobPasses("macOS 15 floor")),
    latest_stable: status(jobPasses("macOS 26 latest stable")),
  };
}

function commandText(executable: string, argumentsList: string[]): string {
  return execFileSync(executable, argumentsList, {
    cwd: workspaceRoot,
    encoding: "utf8",
    env: { ...process.env, LC_ALL: "C" },
    stdio: ["ignore", "pipe", "ignore"],
  }).trim();
}

function commandPasses(executable: string, argumentsList: string[]): void {
  execFileSync(executable, argumentsList, {
    cwd: workspaceRoot,
    env: { ...process.env, LC_ALL: "C" },
    stdio: "ignore",
  });
}

function sha256File(file: string): string {
  return createHash("sha256").update(readFileSync(file)).digest("hex");
}

function appBundleBytes(path: string): number {
  let bytes = 0;
  for (const entry of readdirSync(path, { withFileTypes: true })) {
    const child = join(path, entry.name);
    if (entry.isDirectory()) bytes += appBundleBytes(child);
    else if (entry.isFile() || entry.isSymbolicLink()) bytes += lstatSync(child).size;
    else throw new Error("The app bundle contains an unsupported entry.");
  }
  if (!Number.isSafeInteger(bytes)) {
    throw new Error("The app bundle size is invalid.");
  }
  return bytes;
}

export function parseApprovedPowerState(batteryOutput: string, settingsOutput: string): boolean {
  if (!/Now drawing from 'AC Power'/u.test(batteryOutput)) return false;
  const acStart = settingsOutput.indexOf("AC Power:");
  if (acStart < 0) return false;
  const settingsAfterAc = settingsOutput.slice(acStart + "AC Power:".length);
  const nextProfile = /\n\S[^\n]*Power:/u.exec(settingsAfterAc)?.index;
  const acSettings =
    nextProfile === undefined ? settingsAfterAc : settingsAfterAc.slice(0, nextProfile);
  const lowPowerMode = /^\s*lowpowermode\s+([0-9]+)\s*$/mu.exec(acSettings)?.[1];
  const powerMode = /^\s*powermode\s+([0-9]+)\s*$/mu.exec(acSettings)?.[1];
  return (lowPowerMode ?? powerMode) === "0";
}

function hardwareEnvironment(): MacosReleaseGatesInput["environment"] {
  const hardwareJson = JSON.parse(
    commandText("/usr/sbin/system_profiler", ["SPHardwareDataType", "-json"]),
  ) as unknown;
  const hardware = parseJsonObject(hardwareJson);
  const records = hardware?.SPHardwareDataType;
  const record = Array.isArray(records) ? parseJsonObject(records[0]) : null;
  const model = record?.machine_model;
  const chip = record?.chip_type;
  const memoryBytes = Number(commandText("/usr/sbin/sysctl", ["-n", "hw.memsize"]));
  const macosVersion = commandText("/usr/bin/sw_vers", ["-productVersion"]);
  const battery = commandText("/usr/bin/pmset", ["-g", "batt"]);
  const power = commandText("/usr/bin/pmset", ["-g", "custom"]);
  if (
    typeof model !== "string" ||
    typeof chip !== "string" ||
    !Number.isSafeInteger(memoryBytes) ||
    !macosVersionPattern.test(macosVersion) ||
    !parseApprovedPowerState(battery, power)
  ) {
    throw new Error("The approved macOS measurement environment is absent.");
  }
  return {
    hardware: { model, chip, memory_bytes: memoryBytes },
    power: { source: "AC", low_power_mode: false },
    macos_version: macosVersion,
  };
}

async function ciEvidence(runId: string): Promise<CiEvidence> {
  const headers = {
    Accept: "application/vnd.github+json",
    "X-GitHub-Api-Version": "2022-11-28",
  };
  const runResponse = await fetch(
    `https://api.github.com/repos/${repository}/actions/runs/${runId}`,
    { headers },
  );
  const jobsResponse = await fetch(
    `https://api.github.com/repos/${repository}/actions/runs/${runId}/jobs?per_page=100`,
    { headers },
  );
  if (!runResponse.ok || !jobsResponse.ok) {
    throw new Error("The required GitHub Actions evidence is unavailable.");
  }
  const run = parseJsonObject((await runResponse.json()) as unknown);
  const jobsRoot = parseJsonObject((await jobsResponse.json()) as unknown);
  const jobs = jobsRoot?.jobs;
  if (
    typeof run?.conclusion !== "string" ||
    typeof run.head_sha !== "string" ||
    !Array.isArray(jobs)
  ) {
    throw new Error("The required GitHub Actions evidence is malformed.");
  }
  return {
    conclusion: run.conclusion,
    headSha: run.head_sha,
    jobs: jobs.map((value) => {
      const job = parseJsonObject(value);
      if (typeof job?.name !== "string" || typeof job.conclusion !== "string") {
        throw new Error("The required GitHub Actions evidence is malformed.");
      }
      return { conclusion: job.conclusion, name: job.name };
    }),
  };
}

function requiredRustTest(name: string): boolean {
  const result = commandText("cargo", [
    "test",
    "--manifest-path",
    "apps/desktop/src-tauri/Cargo.toml",
    "--lib",
    name,
    "--",
    "--exact",
  ]);
  if (!/test result: ok\. 1 passed; 0 failed/u.test(result)) {
    throw new Error("A required native preflight test did not run.");
  }
  return true;
}

async function localPreflight(executable: string, fixturePath: string): Promise<LocalPreflight> {
  const positioningClamping = requiredRustTest(
    "tests::clamps_every_monitor_edge_without_panicking_for_an_oversized_panel",
  );
  const currentSpace = requiredRustTest(
    "tests::macos_panel_collection_behavior_matches_the_space_contract",
  );
  commandPasses("bunx", [
    "vitest",
    "run",
    "apps/desktop/src/components/panel/panel-keyboard.test.ts",
  ]);
  const escape = true;

  const driver = new ReleaseArtifactDriver(executable, fixturePath);
  let outsideClick = false;
  let persistedLaunchAtLogin = false;
  let rapidInteraction = false;
  let toggling = false;
  try {
    await driver.waitEvent("menu_bar_ready");

    driver.command("launch_at_login");
    persistedLaunchAtLogin =
      (await driver.waitEvent("launch_at_login_pass")) === "launch_at_login_pass";

    driver.command("show");
    await Promise.all([driver.waitEvent("show_accepted"), driver.waitMetric()]);
    driver.command("show");
    toggling = (await driver.waitEvent("toggled_hidden")) === "toggled_hidden";

    driver.command("rapid");
    const [rapidResult] = await Promise.all([
      driver.waitEvent("rapid_interaction_pass"),
      driver.waitMetric(),
    ]);
    rapidInteraction = rapidResult === "rapid_interaction_pass";
    driver.command("hide");
    await driver.waitEvent("hidden");

    driver.command("show");
    await Promise.all([driver.waitEvent("show_accepted"), driver.waitMetric()]);
    driver.command("outside_click");
    outsideClick =
      (await driver.waitEvent("outside_click_pass")) === "outside_click_pass";
  } finally {
    await driver.stop();
  }

  return {
    currentSpace,
    escape,
    outsideClick,
    persistedLaunchAtLogin,
    positioningClamping,
    rapidInteraction,
    toggling,
  };
}

function delay(milliseconds: number): Promise<void> {
  return new Promise((resolveDelay) => setTimeout(resolveDelay, milliseconds));
}

class SignalQueue<Value> {
  private readonly pending: Value[] = [];
  private readonly waiters: Array<(value: Value) => void> = [];

  push(value: Value): void {
    const waiter = this.waiters.shift();
    if (waiter) waiter(value);
    else this.pending.push(value);
  }

  async take(timeoutMilliseconds: number): Promise<Value> {
    const value = this.pending.shift();
    if (value !== undefined) return value;
    return await new Promise<Value>((resolveValue, rejectValue) => {
      const receive = (received: Value) => {
        clearTimeout(timeout);
        resolveValue(received);
      };
      const timeout = setTimeout(() => {
        const index = this.waiters.indexOf(receive);
        if (index >= 0) this.waiters.splice(index, 1);
        rejectValue(new Error("The release artifact did not answer the harness."));
      }, timeoutMilliseconds);
      this.waiters.push(receive);
    });
  }
}

function childEnvironment(fixturePath: string): NodeJS.ProcessEnv {
  const isolatedHome = dirname(fixturePath);
  const allowedNames = ["LANG", "LC_ALL", "LOGNAME", "PATH", "USER", "__CF_USER_TEXT_ENCODING"];
  const environment: NodeJS.ProcessEnv = {
    CFFIXED_USER_HOME: isolatedHome,
    HOME: isolatedHome,
    TMPDIR: isolatedHome,
    TOUCHGRASS_RELEASE_REFRESH_FIXTURE: fixturePath,
  };
  for (const name of allowedNames) {
    if (process.env[name]) environment[name] = process.env[name];
  }
  return environment;
}

class ReleaseArtifactDriver {
  readonly pid: number;
  private readonly child;
  private readonly exited: Promise<void>;
  private readonly events = new Map<ReleaseGateEvent, SignalQueue<ReleaseGateEvent>>();
  private readonly metrics = new SignalQueue<number>();

  constructor(executable: string, fixturePath: string) {
    this.child = spawn(executable, ["--touchgrass-release-gates"], {
      env: childEnvironment(fixturePath),
      stdio: ["pipe", "pipe", "pipe"],
    });
    if (!this.child.pid) throw new Error("The release artifact did not start.");
    this.pid = this.child.pid;
    this.exited = new Promise((resolveExit) => {
      this.child.once("exit", () => resolveExit());
    });
    this.child.on("error", () => undefined);
    const stdout = createInterface({ input: this.child.stdout });
    stdout.on("line", (line) => {
      const event = parseReleaseGateDriverEvent(line);
      if (event) this.queue(event).push(event);
    });
    const stderr = createInterface({ input: this.child.stderr });
    stderr.on("line", (line) => {
      const metric = parsePanelPaintMetric(line);
      if (metric !== null) this.metrics.push(metric);
    });
  }

  private queue(event: ReleaseGateEvent): SignalQueue<ReleaseGateEvent> {
    let queue = this.events.get(event);
    if (!queue) {
      queue = new SignalQueue<ReleaseGateEvent>();
      this.events.set(event, queue);
    }
    return queue;
  }

  command(
    command: "hide" | "launch_at_login" | "outside_click" | "quit" | "rapid" | "refresh" | "show",
  ): void {
    if (this.child.stdin.destroyed || !this.child.stdin.writable) {
      throw new Error("The release artifact command channel is unavailable.");
    }
    this.child.stdin.write(`${command}\n`);
  }

  waitEvent(event: ReleaseGateEvent): Promise<ReleaseGateEvent> {
    return this.queue(event).take(commandTimeoutMilliseconds);
  }

  waitMetric(): Promise<number> {
    return this.metrics.take(commandTimeoutMilliseconds);
  }

  async stop(): Promise<void> {
    if (this.child.exitCode !== null) return;
    this.command("quit");
    await Promise.race([this.exited, delay(2_000)]);
    if (this.child.exitCode === null) {
      this.child.kill("SIGTERM");
      await Promise.race([this.exited, delay(2_000)]);
    }
    if (this.child.exitCode === null) {
      throw new Error("The release artifact did not stop.");
    }
  }
}

async function processTreeTotals(rootPid: number): Promise<MacosProcessTreeTotals> {
  const { stdout } = await executeFile("/bin/ps", ["-axo", "pid=,ppid=,%cpu=,rss="], {
    env: { ...process.env, LC_ALL: "C" },
    maxBuffer: 4 * 1_024 * 1_024,
  });
  return sumMacosProcessTree(parseMacosProcessTable(stdout), rootPid);
}

async function averageCpuWindow(rootPid: number): Promise<number> {
  const samples: number[] = [];
  const startedAt = performance.now();
  while (performance.now() - startedAt < idleWindowMilliseconds) {
    samples.push((await processTreeTotals(rootPid)).cpuPercent);
    await delay(processSampleMilliseconds);
  }
  if (samples.length === 0) throw new Error("The process-tree CPU sample is absent.");
  return samples.reduce((sum, sample) => sum + sample, 0) / samples.length;
}

async function coldStartupSample(executable: string, fixturePath: string): Promise<number> {
  const startedAt = performance.now();
  const driver = new ReleaseArtifactDriver(executable, fixturePath);
  try {
    await driver.waitEvent("menu_bar_ready");
    return performance.now() - startedAt;
  } finally {
    await driver.stop();
  }
}

async function panelPaintSample(driver: ReleaseArtifactDriver): Promise<number> {
  driver.command("show");
  const metric = await driver.waitMetric();
  driver.command("hide");
  await driver.waitEvent("hidden");
  return metric;
}

async function refreshSample(driver: ReleaseArtifactDriver): Promise<{
  averageCpuPercent: number;
  panelPaintMilliseconds: number;
  peakRssBytes: number;
  recoveryMilliseconds: number;
}> {
  const started = driver.waitEvent("refresh_started");
  driver.command("refresh");
  driver.command("show");
  await started;
  const paint = driver.waitMetric();
  const complete = driver.waitEvent("refresh_complete");
  const totals: MacosProcessTreeTotals[] = [];
  let refreshComplete = false;
  void complete.then(() => {
    refreshComplete = true;
  });
  while (!refreshComplete) {
    totals.push(await processTreeTotals(driver.pid));
    await delay(50);
  }
  await complete;
  if (totals.length === 0) totals.push(await processTreeTotals(driver.pid));
  const completedAt = performance.now();
  let recoveryMilliseconds = 0;
  while (true) {
    const current = await processTreeTotals(driver.pid);
    if (current.cpuPercent <= 1) {
      recoveryMilliseconds = performance.now() - completedAt;
      break;
    }
    if (performance.now() - completedAt > 5_000) {
      recoveryMilliseconds = performance.now() - completedAt;
      break;
    }
    await delay(100);
  }
  const panelPaintMilliseconds = await paint;
  driver.command("hide");
  await driver.waitEvent("hidden");
  return {
    averageCpuPercent: totals.reduce((sum, sample) => sum + sample.cpuPercent, 0) / totals.length,
    panelPaintMilliseconds,
    peakRssBytes: Math.max(...totals.map((sample) => sample.rssBytes)),
    recoveryMilliseconds,
  };
}

function asFiveSamples<Value>(values: Value[]): [Value, Value, Value, Value, Value] {
  if (values.length !== sampleCount) throw new Error("Five samples are required.");
  return values as [Value, Value, Value, Value, Value];
}

async function measure(
  executable: string,
  fixturePath: string,
): Promise<MacosReleaseGatesInput["samples"]> {
  const startup: number[] = [];
  for (let sample = 0; sample < sampleCount; sample += 1) {
    startup.push(await coldStartupSample(executable, fixturePath));
  }

  const panelPaint: number[] = [];
  const settledRss: number[] = [];
  const idleCpu: number[] = [];
  const refreshPaint: number[] = [];
  const refreshCpu: number[] = [];
  const refreshRss: number[] = [];
  const recovery: number[] = [];
  const paintDriver = new ReleaseArtifactDriver(executable, fixturePath);
  try {
    await paintDriver.waitEvent("menu_bar_ready");
    for (let sample = 0; sample < sampleCount; sample += 1) {
      panelPaint.push(await panelPaintSample(paintDriver));
    }
  } finally {
    await paintDriver.stop();
  }

  const idleDriver = new ReleaseArtifactDriver(executable, fixturePath);
  try {
    await idleDriver.waitEvent("menu_bar_ready");
    await panelPaintSample(idleDriver);
    await delay(idleSettleMilliseconds);
    for (let sample = 0; sample < sampleCount; sample += 1) {
      settledRss.push((await processTreeTotals(idleDriver.pid)).rssBytes);
      await delay(processSampleMilliseconds);
    }
    for (let sample = 0; sample < sampleCount; sample += 1) {
      idleCpu.push(await averageCpuWindow(idleDriver.pid));
    }
  } finally {
    await idleDriver.stop();
  }

  for (let sample = 0; sample < sampleCount; sample += 1) {
    const refreshDriver = new ReleaseArtifactDriver(executable, fixturePath);
    try {
      await refreshDriver.waitEvent("menu_bar_ready");
      const result = await refreshSample(refreshDriver);
      refreshPaint.push(result.panelPaintMilliseconds);
      refreshCpu.push(result.averageCpuPercent);
      refreshRss.push(result.peakRssBytes);
      recovery.push(result.recoveryMilliseconds);
    } finally {
      await refreshDriver.stop();
    }
  }
  return {
    startup_ms: asFiveSamples(startup),
    panel_paint_ms: asFiveSamples(panelPaint),
    idle_cpu_percent: asFiveSamples(idleCpu),
    settled_rss_bytes: asFiveSamples(settledRss),
    refresh: {
      panel_paint_ms: asFiveSamples(refreshPaint),
      average_cpu_percent: asFiveSamples(refreshCpu),
      peak_rss_bytes: asFiveSamples(refreshRss),
      recovery_to_idle_ms: asFiveSamples(recovery),
    },
  };
}

async function run(argumentsList: readonly string[]): Promise<void> {
  if (process.platform !== "darwin" || process.arch !== "arm64") {
    throw new Error("The macOS Apple-silicon measurement host is required.");
  }
  const arguments_ = parseMacosReleaseGateArguments(argumentsList);
  const dmgPath = resolve(arguments_.dmg);
  const trustPath = resolve(arguments_.trust);
  const outputPath = resolve(arguments_.output);
  if (outputPath === dmgPath || outputPath === trustPath || existsSync(outputPath)) {
    throw new Error("The output receipt must use a new file.");
  }

  const temporaryDirectory = mkdtempSync(join(tmpdir(), "touchgrass-release-gates-"));
  const mountPoint = join(temporaryDirectory, "volume");
  const fixturePath = join(temporaryDirectory, "refresh-fixture.json");
  mkdirSync(mountPoint);
  let mounted = false;
  try {
    const fixtureBytes = generateMacosRefreshFixtureBytes();
    writeFileSync(fixturePath, fixtureBytes, { mode: 0o600 });
    mkdirSync(join(temporaryDirectory, "Library"));
    const fixture = createMacosRefreshFixtureBinding(fixtureBytes);
    const dmgStatistics = lstatSync(dmgPath);
    if (!dmgStatistics.isFile()) throw new Error("The release DMG is invalid.");
    commandPasses("/usr/bin/hdiutil", [
      "attach",
      "-readonly",
      "-nobrowse",
      "-mountpoint",
      mountPoint,
      dmgPath,
    ]);
    mounted = true;
    const apps = readdirSync(mountPoint, { withFileTypes: true }).filter((entry) =>
      entry.name.endsWith(".app"),
    );
    if (apps.length !== 1 || !apps[0]?.isDirectory()) {
      throw new Error("The release DMG must contain one app.");
    }
    const appPath = join(mountPoint, apps[0].name);
    const executable = join(appPath, "Contents", "MacOS", "touchgrassbar");
    const appVersion = commandText("/usr/bin/plutil", [
      "-extract",
      "CFBundleShortVersionString",
      "raw",
      "-o",
      "-",
      join(appPath, "Contents", "Info.plist"),
    ]);
    if (commandText("/usr/bin/lipo", ["-archs", executable]) !== "arm64") {
      throw new Error("The embedded release app is not Apple silicon only.");
    }
    const binding = bindReleaseCandidate({
      appBytes: appBundleBytes(appPath),
      appVersion,
      dmgBytes: dmgStatistics.size,
      dmgName: basename(dmgPath),
      dmgSha256: sha256File(dmgPath),
      receipt: JSON.parse(readFileSync(trustPath, "utf8")) as unknown,
    });
    const currentCommit = commandText("git", ["rev-parse", "HEAD"]);
    const sourceChanges = commandText("git", ["status", "--porcelain"]);
    if (currentCommit !== binding.candidate.commit || sourceChanges !== "") {
      throw new Error("The source checkout does not match the measured candidate.");
    }
    const environment = hardwareEnvironment();
    if (
      environment.hardware.chip !== "Apple M4 Pro" ||
      environment.hardware.memory_bytes !== REFERENCE_MEMORY_BYTES
    ) {
      throw new Error("The approved M4 Pro 24 GB reference host is required.");
    }
    const automatedPreflight = buildAutomatedPreflight(
      await localPreflight(executable, fixturePath),
      await ciEvidence(binding.mainCiRunId),
      binding.candidate.commit,
    );
    const input: MacosReleaseGatesInput = {
      schema_version: MACOS_RELEASE_GATES_SCHEMA_VERSION,
      candidate: binding.candidate,
      environment,
      fixture,
      samples: await measure(executable, fixturePath),
      automated_preflight: automatedPreflight,
    };
    const report = evaluateMacosReleaseGates(input);
    writeFileSync(outputPath, `${JSON.stringify(report, null, 2)}\n`, {
      encoding: "utf8",
      flag: "wx",
      mode: 0o644,
    });
    if (report.status !== "PASS") {
      throw new Error("macOS release gates: FAIL.");
    }
    console.log("macOS release gates: PASS.");
  } finally {
    if (mounted) {
      try {
        commandPasses("/usr/bin/hdiutil", ["detach", mountPoint]);
      } catch {
        // Keep the primary fail-closed result.
      }
    }
    rmSync(temporaryDirectory, { force: true, recursive: true });
  }
}

if (import.meta.main) {
  try {
    await run(process.argv.slice(2));
  } catch (error) {
    const message =
      error instanceof Error && error.message.startsWith("Usage:")
        ? error.message
        : "macOS release gates: FAIL.";
    console.error(message);
    process.exitCode = 1;
  }
}
