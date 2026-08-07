#!/usr/bin/env bun

import { execFileSync, spawnSync } from "node:child_process";
import {
  appendFileSync,
  mkdirSync,
  readFileSync,
  writeFileSync,
} from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const releaseRepository = "FabienGreard/TouchGrassBar";
const stableUpdaterEndpoint =
  "https://github.com/FabienGreard/TouchGrassBar/releases/latest/download/latest.json";
const stableTagPattern = /^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$/;
const scriptDirectory = dirname(fileURLToPath(import.meta.url));
const governance = JSON.parse(
  readFileSync(
    resolve(scriptDirectory, "..", ".github", "release-governance.json"),
    "utf8",
  ),
) as {
  environments: {
    "macos-release": { secrets: string[]; variables: string[] };
  };
};
const releaseSecrets = governance.environments["macos-release"].secrets;
const releaseEnvironmentVariables =
  governance.environments["macos-release"].variables;

const publicConfigurationNames = [
  "TAURI_UPDATER_ENDPOINT",
  "TAURI_UPDATER_PUBLIC_KEY",
] as const;

type ProtectedConfigurationName =
  | (typeof releaseSecrets)[number]
  | (typeof releaseEnvironmentVariables)[number];
type PublicConfigurationName = (typeof publicConfigurationNames)[number];
type PresenceState = "present" | "absent";

type PresenceReceiptInput = {
  capturedAt: string;
  commit: string;
  protectedStates: Record<string, boolean>;
  publicConfiguration: Record<PublicConfigurationName, boolean>;
  tag: string;
  workflowRunId: string;
};

type LatestManifestInput = {
  notes: string;
  pubDate: string;
  signature: string;
  tag: string;
  updaterArchiveName: string;
};

function parseStableReleaseTag(tag: string) {
  const match = stableTagPattern.exec(tag);
  if (!match) {
    throw new Error("Release tag must have exact vMAJOR.MINOR.PATCH form.");
  }
  return { tag, version: tag.slice(1) };
}

function releaseAssetNames(tag: string) {
  const { version } = parseStableReleaseTag(tag);
  const updaterArchive = `TouchGrassBar_${version}_aarch64.app.tar.gz`;
  return {
    checksums: "SHA256SUMS",
    dmg: `TouchGrassBar_${version}_aarch64.dmg`,
    latest: "latest.json",
    receipt: `release-trust-${version}.json`,
    updaterArchive,
    updaterSignature: `${updaterArchive}.sig`,
  };
}

function presenceEntry(
  name: ProtectedConfigurationName | PublicConfigurationName,
  scope: "environment:macos-release" | "repository",
  present: boolean,
) {
  return {
    name,
    scope,
    state: (present ? "present" : "absent") satisfies PresenceState,
  };
}

function validIsoTimestamp(value: string) {
  const parsed = Date.parse(value);
  return Number.isFinite(parsed) && new Date(parsed).toISOString() === value;
}

function createPresenceReceipt({
  capturedAt,
  commit,
  protectedStates,
  publicConfiguration,
  tag,
  workflowRunId,
}: PresenceReceiptInput) {
  const candidate = parseStableReleaseTag(tag);
  if (!/^[0-9a-f]{40}$/.test(commit)) {
    throw new Error("Release commit must be a full lowercase SHA.");
  }
  if (!/^[1-9][0-9]*$/.test(workflowRunId)) {
    throw new Error("Workflow run ID is invalid.");
  }
  if (!validIsoTimestamp(capturedAt)) {
    throw new Error("Configuration capture time must be an exact ISO timestamp.");
  }

  const secrets = releaseSecrets.map((name) =>
    presenceEntry(name, "environment:macos-release", protectedStates[name] === true),
  );
  const variables = releaseEnvironmentVariables.map((name) =>
    presenceEntry(name, "environment:macos-release", protectedStates[name] === true),
  );
  const publicEntries = publicConfigurationNames.map((name) =>
    presenceEntry(name, "repository", publicConfiguration[name] === true),
  );
  const absent = [...secrets, ...variables, ...publicEntries]
    .filter((entry) => entry.state === "absent")
    .map((entry) => entry.name);
  if (absent.length > 0) {
    throw new Error(`Release configuration is absent: ${absent.join(", ")}.`);
  }

  return {
    schema_version: "touchgrass.release-configuration.v1" as const,
    candidate: {
      ...candidate,
      commit,
      workflow_run_id: workflowRunId,
    },
    captured_at: capturedAt,
    environments: [
      {
        name: "macos-release" as const,
        secrets,
        variables,
      },
      {
        name: "public-release" as const,
        secrets: [],
        variables: [],
      },
    ],
    public_configuration: publicEntries,
    redaction: {
      protected_values_received: false,
      protected_values_emitted: false,
    },
  };
}

function safeUpdaterAssetName(name: string) {
  return (
    /^[A-Za-z0-9][A-Za-z0-9._-]*$/.test(name) &&
    !name.includes("..") &&
    name.endsWith(".app.tar.gz")
  );
}

function createLatestManifest({
  notes,
  pubDate,
  signature,
  tag,
  updaterArchiveName,
}: LatestManifestInput) {
  const { version } = parseStableReleaseTag(tag);
  if (!validIsoTimestamp(pubDate)) {
    throw new Error("Updater publication time must be an exact ISO timestamp.");
  }
  if (!safeUpdaterAssetName(updaterArchiveName)) {
    throw new Error("Updater archive name is unsafe.");
  }
  if (signature.trim() !== signature || signature.length === 0) {
    throw new Error("Updater signature is invalid.");
  }
  if (notes.trim() !== notes || notes.length === 0) {
    throw new Error("Updater notes are invalid.");
  }

  return {
    notes,
    platforms: {
      "darwin-aarch64": {
        signature,
        url: `https://github.com/${releaseRepository}/releases/download/${tag}/${updaterArchiveName}`,
      },
    },
    pub_date: pubDate,
    version,
  };
}

function requiredEnvironment(name: string) {
  const value = Bun.env[name]?.trim();
  if (!value) throw new Error(`Required release input is absent: ${name}.`);
  return value;
}

function run(executable: string, argumentsList: string[]) {
  try {
    return execFileSync(executable, argumentsList, {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"],
    }).trim();
  } catch {
    throw new Error(`Release source check failed: ${argumentsList[0] ?? executable}.`);
  }
}

function sourceIsOnMain(commit: string) {
  const fetched = spawnSync(
    "git",
    ["fetch", "--no-tags", "origin", "+refs/heads/main:refs/remotes/origin/main"],
    { stdio: "ignore" },
  );
  if (fetched.status !== 0) {
    throw new Error("Release source check failed: main cannot be fetched.");
  }
  const membership = spawnSync(
    "git",
    ["merge-base", "--is-ancestor", commit, "refs/remotes/origin/main"],
    { stdio: "ignore" },
  );
  if (membership.status !== 0) {
    throw new Error("Release tag commit is not a member of main.");
  }
}

function successfulMainRun(repository: string, commit: string) {
  const result = spawnSync(
    "gh",
    [
      "run",
      "list",
      "--repo",
      repository,
      "--workflow",
      ".github/workflows/ci.yml",
      "--branch",
      "main",
      "--event",
      "push",
      "--commit",
      commit,
      "--status",
      "success",
      "--limit",
      "1",
      "--json",
      "conclusion,databaseId,headSha",
    ],
    { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] },
  );
  if (result.status !== 0) {
    throw new Error("The exact main CI result cannot be checked.");
  }
  const runs = JSON.parse(result.stdout) as Array<{
    conclusion?: unknown;
    databaseId?: unknown;
    headSha?: unknown;
  }>;
  const ciRun = runs[0];
  if (
    ciRun?.conclusion !== "success" ||
    ciRun.headSha !== commit ||
    typeof ciRun.databaseId !== "number"
  ) {
    throw new Error("Release tag commit has no successful exact-head main CI run.");
  }
  return String(ciRun.databaseId);
}

function assertReleaseDoesNotExist(repository: string, tag: string) {
  const result = spawnSync(
    "gh",
    [
      "api",
      "--paginate",
      `repos/${repository}/releases?per_page=100`,
      "--jq",
      ".[].tag_name",
    ],
    { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] },
  );
  if (result.status !== 0) {
    throw new Error("Existing GitHub Releases cannot be checked.");
  }
  if (result.stdout.split(/\r?\n/u).includes(tag)) {
    throw new Error("The immutable release tag already has a GitHub Release.");
  }
}

function writeWorkflowOutput(name: string, value: string) {
  const outputPath = requiredEnvironment("GITHUB_OUTPUT");
  appendFileSync(outputPath, `${name}=${value}\n`, { encoding: "utf8" });
}

function validateSource() {
  const tag = requiredEnvironment("RELEASE_TAG");
  const commit = requiredEnvironment("RELEASE_COMMIT");
  const repository = requiredEnvironment("GITHUB_REPOSITORY");
  const candidate = parseStableReleaseTag(tag);
  if (!/^[0-9a-f]{40}$/.test(commit)) {
    throw new Error("Release commit must be a full lowercase SHA.");
  }
  if (repository !== releaseRepository) {
    throw new Error("Release repository is invalid.");
  }
  const head = run("git", ["rev-parse", "HEAD"]);
  const tagCommit = run("git", ["rev-parse", `refs/tags/${tag}^{commit}`]);
  if (head !== commit || tagCommit !== commit) {
    throw new Error("Release tag, checkout, and workflow commit do not match.");
  }
  sourceIsOnMain(commit);
  const ciRunId = successfulMainRun(repository, commit);
  assertReleaseDoesNotExist(repository, tag);
  writeWorkflowOutput("ci_run_id", ciRunId);
  writeWorkflowOutput("commit", commit);
  writeWorkflowOutput("tag", tag);
  writeWorkflowOutput("version", candidate.version);
  console.log(`Release source: ${tag} at ${commit} with main CI ${ciRunId}.`);
}

function checkedPublicConfiguration() {
  const configPath = resolve(
    scriptDirectory,
    "..",
    "apps",
    "desktop",
    "src-tauri",
    "tauri.conf.json",
  );
  const config = JSON.parse(readFileSync(configPath, "utf8")) as {
    plugins?: { updater?: { endpoints?: unknown; pubkey?: unknown } };
  };
  const updater = config.plugins?.updater;
  const endpointPresent =
    Array.isArray(updater?.endpoints) &&
    updater.endpoints.length === 1 &&
    updater.endpoints[0] === stableUpdaterEndpoint;
  const publicKeyPresent =
    typeof updater?.pubkey === "string" &&
    updater.pubkey.trim().length > 0 &&
    !updater.pubkey.includes("NOT_CONFIGURED");
  return {
    TAURI_UPDATER_ENDPOINT: endpointPresent,
    TAURI_UPDATER_PUBLIC_KEY: publicKeyPresent,
  } satisfies Record<PublicConfigurationName, boolean>;
}

function protectedConfigurationStates() {
  return Object.fromEntries(
    [...releaseSecrets, ...releaseEnvironmentVariables].map((name) => [
      name,
      Bun.env[`RELEASE_HAS_${name}`] === "true",
    ]),
  );
}

function writePresenceReceipt() {
  const outputDirectory = resolve(scriptDirectory, "..", "release-output");
  const receipt = createPresenceReceipt({
    capturedAt: new Date().toISOString(),
    commit: requiredEnvironment("RELEASE_COMMIT"),
    protectedStates: protectedConfigurationStates(),
    publicConfiguration: checkedPublicConfiguration(),
    tag: requiredEnvironment("RELEASE_TAG"),
    workflowRunId: requiredEnvironment("GITHUB_RUN_ID"),
  });
  mkdirSync(outputDirectory, { recursive: true });
  writeFileSync(
    resolve(outputDirectory, "release-configuration.json"),
    `${JSON.stringify(receipt, null, 2)}\n`,
    { encoding: "utf8", mode: 0o644 },
  );
  const count = releaseSecrets.length + releaseEnvironmentVariables.length + 2;
  console.log(`Release configuration presence: verified (${count} names).`);
}

function commandLine() {
  const command = process.argv[2];
  if (process.argv.length !== 3 || !command) {
    throw new Error("Use validate-source or presence.");
  }
  if (command === "validate-source") return validateSource();
  if (command === "presence") return writePresenceReceipt();
  throw new Error(`Unknown release contract command: ${command}.`);
}

if (import.meta.main) commandLine();

export {
  createLatestManifest,
  createPresenceReceipt,
  parseStableReleaseTag,
  publicConfigurationNames,
  releaseAssetNames,
  releaseEnvironmentVariables,
  releaseRepository,
  releaseSecrets,
  stableUpdaterEndpoint,
};
