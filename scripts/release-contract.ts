#!/usr/bin/env bun

import { execFileSync, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  appendFileSync,
  existsSync,
  lstatSync,
  mkdirSync,
  readFileSync,
  writeFileSync,
} from "node:fs";
import { dirname, isAbsolute, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const releaseRepository = "FabienGreard/TouchGrassBar";
const stableUpdaterEndpoint =
  "https://github.com/FabienGreard/TouchGrassBar/releases/latest/download/latest.json";
const stableTagPattern = /^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$/;
const scriptDirectory = dirname(fileURLToPath(import.meta.url));
const databaseFixtureManifestPath = resolve(
  scriptDirectory,
  "..",
  "apps",
  "desktop",
  "src-tauri",
  "tests",
  "fixtures",
  "releases",
  "manifest.json",
);
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

type DatabaseFixtureEntry = {
  database: string;
  sha256: string;
  tag: string;
};

type DatabaseFixtureManifestEntry = DatabaseFixtureEntry & {
  releaseStatus: "candidate" | "official";
  sourceCommit: string;
};

type DatabaseCompatibilityEvidenceInput = {
  capturedAt: string;
  commit: string;
  fixtures: DatabaseFixtureEntry[];
  tag: string;
  workflowRunId: string;
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
    databaseCompatibility: `database-compatibility-${version}.json`,
    dmg: `TouchGrassBar_${version}_aarch64.dmg`,
    latest: "latest.json",
    receipt: `release-trust-${version}.json`,
    updaterArchive,
    updaterSignature: `${updaterArchive}.sig`,
  };
}

function safeDatabaseFixturePath(tag: string, database: string) {
  const components = database.split("/");
  return (
    database.length <= 180 &&
    !isAbsolute(database) &&
    !database.includes("\\") &&
    /^[A-Za-z0-9._/-]+$/u.test(database) &&
    components.every(
      (component) =>
        component.length > 0 && component !== "." && component !== "..",
    ) &&
    components[0] === tag &&
    database.endsWith(".sqlite3")
  );
}

function parseDatabaseFixtureManifest(input: unknown, candidateTag: string) {
  parseStableReleaseTag(candidateTag);
  if (typeof input !== "object" || input === null || Array.isArray(input)) {
    throw new Error("Database fixture manifest is invalid.");
  }
  const manifest = input as { fixtures?: unknown; formatVersion?: unknown };
  if (manifest.formatVersion !== 1) {
    throw new Error("Database fixture manifest version is invalid.");
  }
  const fixtures = manifest.fixtures;
  if (!Array.isArray(fixtures) || fixtures.length === 0) {
    throw new Error("Database fixture manifest has no fixtures.");
  }

  const tags = new Set<string>();
  const databases = new Set<string>();
  const entries = fixtures.map((fixture) => {
    if (
      typeof fixture !== "object" ||
      fixture === null ||
      Array.isArray(fixture)
    ) {
      throw new Error("Database fixture manifest entry is invalid.");
    }
    const { database, releaseStatus, sha256, sourceCommit, tag } =
      fixture as Record<string, unknown>;
    if (
      typeof tag !== "string" ||
      typeof database !== "string" ||
      typeof sha256 !== "string" ||
      (releaseStatus !== "candidate" && releaseStatus !== "official") ||
      typeof sourceCommit !== "string" ||
      (releaseStatus === "official" && !/^[0-9a-f]{40}$/u.test(sourceCommit)) ||
      (releaseStatus === "candidate" && sourceCommit !== "candidate") ||
      !safeDatabaseFixturePath(tag, database) ||
      !/^[0-9a-f]{64}$/u.test(sha256)
    ) {
      throw new Error("Database fixture manifest entry is invalid.");
    }
    parseStableReleaseTag(tag);
    if (tags.has(tag) || databases.has(database)) {
      throw new Error("Database fixture manifest has a duplicate entry.");
    }
    tags.add(tag);
    databases.add(database);
    return {
      database,
      releaseStatus,
      sha256,
      sourceCommit,
      tag,
    } satisfies DatabaseFixtureManifestEntry;
  });

  const candidateTags = entries
    .filter((entry) => entry.releaseStatus === "candidate")
    .map((entry) => entry.tag);
  if (candidateTags.length !== 1 || candidateTags[0] !== candidateTag) {
    throw new Error(
      `Database fixture manifest must have only candidate ${candidateTag}.`,
    );
  }
  return entries;
}

function assertDatabaseFixtureReleaseSet(
  fixtures: readonly DatabaseFixtureManifestEntry[],
  candidateTag: string,
  publishedStableTags: readonly string[],
) {
  const officialTags = fixtures
    .filter((fixture) => fixture.releaseStatus === "official")
    .map((fixture) => fixture.tag)
    .sort();
  const publishedTags = [...publishedStableTags].sort();
  if (
    new Set(publishedTags).size !== publishedTags.length ||
    publishedTags.some((tag) => !stableTagPattern.test(tag)) ||
    JSON.stringify(officialTags) !== JSON.stringify(publishedTags)
  ) {
    throw new Error(
      "Official database fixtures do not match published stable GitHub Releases.",
    );
  }
  const candidates = fixtures.filter(
    (fixture) => fixture.releaseStatus === "candidate",
  );
  if (candidates.length !== 1 || candidates[0]?.tag !== candidateTag) {
    throw new Error(
      `Database fixture manifest must have only candidate ${candidateTag}.`,
    );
  }
}

function publishedStableReleaseTags(repository: string) {
  const result = spawnSync(
    "gh",
    [
      "api",
      "--paginate",
      `repos/${repository}/releases?per_page=100`,
      "--jq",
      ".[] | select(.draft == false and .prerelease == false) | .tag_name",
    ],
    { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] },
  );
  if (result.status !== 0) {
    throw new Error("Published stable GitHub Releases cannot be checked.");
  }
  return result.stdout
    .split(/\r?\n/u)
    .filter((tag) => stableTagPattern.test(tag));
}

function verifyDatabaseFixtureCandidate(
  tag: string,
  publishedStableTags?: readonly string[],
) {
  if (!existsSync(databaseFixtureManifestPath)) {
    throw new Error("Database fixture manifest is absent.");
  }
  let input: unknown;
  try {
    input = JSON.parse(readFileSync(databaseFixtureManifestPath, "utf8"));
  } catch {
    throw new Error("Database fixture manifest is unreadable.");
  }
  const fixtures = parseDatabaseFixtureManifest(input, tag);
  if (publishedStableTags !== undefined) {
    assertDatabaseFixtureReleaseSet(fixtures, tag, publishedStableTags);
  }
  const fixtureDirectory = dirname(databaseFixtureManifestPath);
  for (const fixture of fixtures) {
    const databasePath = resolve(fixtureDirectory, fixture.database);
    const localPath = relative(fixtureDirectory, databasePath);
    if (
      localPath.startsWith("..") ||
      isAbsolute(localPath) ||
      !existsSync(databasePath) ||
      !lstatSync(databasePath).isFile()
    ) {
      throw new Error("A database compatibility fixture is absent.");
    }
    const actual = createHash("sha256")
      .update(readFileSync(databasePath))
      .digest("hex");
    if (actual !== fixture.sha256) {
      throw new Error("A database compatibility fixture checksum is invalid.");
    }
  }
  return fixtures.map(({ database, sha256, tag: fixtureTag }) => ({
    database,
    sha256,
    tag: fixtureTag,
  }));
}

function createDatabaseCompatibilityEvidence({
  capturedAt,
  commit,
  fixtures,
  tag,
  workflowRunId,
}: DatabaseCompatibilityEvidenceInput) {
  const candidate = parseStableReleaseTag(tag);
  if (
    !/^[0-9a-f]{40}$/u.test(commit) ||
    !/^[1-9][0-9]*$/u.test(workflowRunId) ||
    !validIsoTimestamp(capturedAt) ||
    !fixtures.some((fixture) => fixture.tag === tag)
  ) {
    throw new Error("Database compatibility evidence identity is invalid.");
  }
  return {
    schema_version: "touchgrass.database-compatibility.v1" as const,
    candidate: {
      ...candidate,
      commit,
      workflow_run_id: workflowRunId,
    },
    captured_at: capturedAt,
    fixtures: fixtures.map((fixture) => ({
      ...fixture,
      result: "PASS" as const,
    })),
    verification: {
      result: "PASS" as const,
      test: "database::release_compatibility::" as const,
    },
    redaction: {
      private_paths: "ABSENT" as const,
      sensitive_values: "ABSENT" as const,
    },
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
    throw new Error(
      "Configuration capture time must be an exact ISO timestamp.",
    );
  }

  const secrets = releaseSecrets.map((name) =>
    presenceEntry(
      name,
      "environment:macos-release",
      protectedStates[name] === true,
    ),
  );
  const variables = releaseEnvironmentVariables.map((name) =>
    presenceEntry(
      name,
      "environment:macos-release",
      protectedStates[name] === true,
    ),
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
    throw new Error(
      `Release source check failed: ${argumentsList[0] ?? executable}.`,
    );
  }
}

function sourceIsOnMain(commit: string) {
  const fetched = spawnSync(
    "git",
    [
      "fetch",
      "--no-tags",
      "origin",
      "+refs/heads/main:refs/remotes/origin/main",
    ],
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
    throw new Error(
      "Release tag commit has no successful exact-head main CI run.",
    );
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
  verifyDatabaseFixtureCandidate(tag, publishedStableReleaseTags(repository));
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

function writeDatabaseCompatibilityEvidence() {
  const tag = requiredEnvironment("RELEASE_TAG");
  const fixtures = verifyDatabaseFixtureCandidate(tag);
  const evidence = createDatabaseCompatibilityEvidence({
    capturedAt: new Date().toISOString(),
    commit: requiredEnvironment("RELEASE_COMMIT"),
    fixtures,
    tag,
    workflowRunId: requiredEnvironment("GITHUB_RUN_ID"),
  });
  const outputDirectory = resolve(scriptDirectory, "..", "release-output");
  mkdirSync(outputDirectory, { recursive: true });
  writeFileSync(
    resolve(outputDirectory, releaseAssetNames(tag).databaseCompatibility),
    `${JSON.stringify(evidence, null, 2)}\n`,
    { encoding: "utf8", mode: 0o644 },
  );
  console.log(
    `Database compatibility: PASS (${fixtures.length} release fixtures).`,
  );
}

function commandLine() {
  const command = process.argv[2];
  if (process.argv.length !== 3 || !command) {
    throw new Error("Use validate-source, presence, or database-evidence.");
  }
  if (command === "validate-source") return validateSource();
  if (command === "presence") return writePresenceReceipt();
  if (command === "database-evidence") {
    return writeDatabaseCompatibilityEvidence();
  }
  throw new Error(`Unknown release contract command: ${command}.`);
}

if (import.meta.main) commandLine();

export {
  assertDatabaseFixtureReleaseSet,
  createDatabaseCompatibilityEvidence,
  createLatestManifest,
  createPresenceReceipt,
  parseDatabaseFixtureManifest,
  parseStableReleaseTag,
  publicConfigurationNames,
  publishedStableReleaseTags,
  releaseAssetNames,
  releaseEnvironmentVariables,
  releaseRepository,
  releaseSecrets,
  stableUpdaterEndpoint,
  verifyDatabaseFixtureCandidate,
};
