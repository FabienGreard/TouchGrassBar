#!/usr/bin/env bun

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import {
  createLatestManifest,
  parseStableReleaseTag,
  releaseAssetNames,
} from "./release-contract";

type ArtifactRecord = {
  bytes: number;
  name: string;
  sha256: string;
};

type SignatureFacts = {
  cdhash: string;
  hardenedRuntime: true;
  identity: string;
  teamIdentifier: string;
  timestamped: true;
};

type TrustReceiptInput = {
  actions: Array<{ action: string; ref: string }>;
  artifacts: ArtifactRecord[];
  capturedAt: string;
  certificateSha256: string;
  commit: string;
  configuration: unknown;
  governance: unknown;
  governanceSha256: string;
  identity: string;
  mainCiRunId: string;
  tag: string;
  teamIdentifier: string;
  toolchains: {
    bun: string;
    cargo: string;
    notarytool: string;
    rustc: string;
    xcode: string;
  };
  workflowRunId: string;
};

const workspaceRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const tauriRoot = join(workspaceRoot, "apps", "desktop", "src-tauri");
const bundleRoot = join(tauriRoot, "target", "release", "bundle");
const tauriConfigPath = join(tauriRoot, "tauri.conf.json");
const outputDirectory = join(workspaceRoot, "release-output");

function requiredEnvironment(name: string) {
  const value = Bun.env[name]?.trim();
  if (!value) throw new Error(`Required release input is absent: ${name}.`);
  return value;
}

function validSha256(value: string) {
  return /^[0-9a-f]{64}$/.test(value);
}

function parseCodeSignatureDetails(details: string, expectedIdentity: string) {
  const identity = /^Authority=(.+)$/mu.exec(details)?.[1];
  const teamIdentifier = /^TeamIdentifier=([A-Z0-9]{10})$/mu.exec(details)?.[1];
  const cdhash = /^CDHash=([0-9a-f]{40,64})$/mu.exec(details)?.[1];
  const timestamped = /^Timestamp=.+$/mu.test(details);
  const hardenedRuntime = /\bflags=0x[0-9a-f]+\(runtime\)/u.test(details);
  const identifier = /^Identifier=app\.touchgrass\.bar$/mu.test(details);
  if (
    identity !== expectedIdentity ||
    !teamIdentifier ||
    !cdhash ||
    !timestamped ||
    !hardenedRuntime ||
    !identifier
  ) {
    throw new Error("The app signature trust contract is incomplete.");
  }
  return {
    cdhash,
    hardenedRuntime: true,
    identity,
    teamIdentifier,
    timestamped: true,
  } satisfies SignatureFacts;
}

function validateUpdaterArchiveEntries(entries: string[]) {
  const normalizedEntries = entries.filter((entry) => entry.length > 0);
  const binary = "TouchGrassBar.app/Contents/MacOS/touchgrassbar";
  const valid =
    normalizedEntries.length > 0 &&
    normalizedEntries.includes(binary) &&
    normalizedEntries.every((entry) => {
      const components = entry.split("/");
      return (
        !entry.startsWith("/") &&
        !components.includes("..") &&
        !components.includes(".") &&
        (entry === "TouchGrassBar.app" ||
          entry.startsWith("TouchGrassBar.app/"))
      );
    });
  if (!valid) throw new Error("Updater archive contents are invalid.");
}

function formatChecksums(records: ArtifactRecord[]) {
  return [...records]
    .sort((left, right) => left.name.localeCompare(right.name))
    .map((record) => {
      if (
        !validSha256(record.sha256) ||
        record.bytes < 0 ||
        !Number.isSafeInteger(record.bytes) ||
        !/^[A-Za-z0-9][A-Za-z0-9._-]*$/.test(record.name)
      ) {
        throw new Error("Artifact checksum record is invalid.");
      }
      return `${record.sha256}  ${record.name}\n`;
    })
    .join("");
}

function createTrustReceipt({
  actions,
  artifacts,
  capturedAt,
  certificateSha256,
  commit,
  configuration,
  governance,
  governanceSha256,
  identity,
  mainCiRunId,
  tag,
  teamIdentifier,
  toolchains,
  workflowRunId,
}: TrustReceiptInput) {
  const candidate = parseStableReleaseTag(tag);
  if (
    !/^[0-9a-f]{40}$/.test(commit) ||
    !/^[1-9][0-9]*$/.test(mainCiRunId) ||
    !/^[1-9][0-9]*$/.test(workflowRunId) ||
    !/^[A-Z0-9]{10}$/.test(teamIdentifier) ||
    !validSha256(certificateSha256) ||
    !validSha256(governanceSha256) ||
    new Date(capturedAt).toISOString() !== capturedAt
  ) {
    throw new Error("Release trust receipt identity is invalid.");
  }
  if (
    actions.length === 0 ||
    actions.some(
      ({ action, ref }) =>
        !/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(action) ||
        !/^(?:stable|v[0-9]+(?:\.[0-9]+){0,2})$/.test(ref),
    )
  ) {
    throw new Error("Release Action versions are invalid.");
  }
  formatChecksums(artifacts);

  return {
    schema_version: "touchgrass.release-trust.v1" as const,
    candidate: {
      ...candidate,
      commit,
      main_ci_run_id: mainCiRunId,
      workflow_run_id: workflowRunId,
    },
    captured_at: capturedAt,
    actions,
    toolchains,
    configuration,
    governance: {
      contract: governance,
      sha256: governanceSha256,
    },
    artifacts,
    signing: {
      identity,
      team_identifier: teamIdentifier,
      public_certificate_sha256: certificateSha256,
    },
    distribution_trust: {
      app: {
        architecture: "arm64" as const,
        gatekeeper: "PASS" as const,
        hardened_runtime: "PASS" as const,
        notarization: "PASS" as const,
        stapling: "PASS" as const,
        timestamp: "PASS" as const,
      },
      dmg: {
        gatekeeper: "PASS" as const,
        notarization: "PASS" as const,
        stapling: "PASS" as const,
      },
      updater_signature: "PASS" as const,
    },
    redaction: {
      credential_material: "ABSENT" as const,
      private_paths: "ABSENT" as const,
      raw_provider_responses: "ABSENT" as const,
      runner_paths: "ABSENT" as const,
    },
  };
}

function commandText(
  executable: string,
  argumentsList: string[],
  failureMessage: string,
) {
  const result = spawnSync(executable, argumentsList, {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
  });
  if (result.status !== 0) throw new Error(failureMessage);
  return `${result.stdout}${result.stderr}`.trim();
}

function commandPasses(
  executable: string,
  argumentsList: string[],
  failureMessage: string,
) {
  const result = spawnSync(executable, argumentsList, { stdio: "ignore" });
  if (result.status !== 0) throw new Error(failureMessage);
}

function sha256File(filePath: string) {
  return createHash("sha256").update(readFileSync(filePath)).digest("hex");
}

function artifactRecord(filePath: string): ArtifactRecord {
  const statistics = statSync(filePath);
  if (!statistics.isFile()) throw new Error("Release artifact is not a file.");
  return {
    bytes: statistics.size,
    name: basename(filePath),
    sha256: sha256File(filePath),
  };
}

function extractCertificateSha256(appPath: string, temporaryDirectory: string) {
  const certificateDirectory = mkdtempSync(
    join(temporaryDirectory, "certificate-"),
  );
  const certificatePrefix = join(certificateDirectory, "leaf-");
  commandPasses(
    "/usr/bin/codesign",
    ["--display", `--extract-certificates=${certificatePrefix}`, appPath],
    "The public signing certificate cannot be extracted.",
  );
  const leafCertificatePath = `${certificatePrefix}0`;
  if (!existsSync(leafCertificatePath)) {
    throw new Error("The public signing certificate is absent.");
  }
  return sha256File(leafCertificatePath);
}

function verifyApp(
  appPath: string,
  expectedIdentity: string,
  expectedVersion: string,
  temporaryDirectory: string,
) {
  commandPasses(
    "/usr/bin/codesign",
    ["--verify", "--deep", "--strict", appPath],
    "The app code signature is invalid.",
  );
  const details = commandText(
    "/usr/bin/codesign",
    ["--display", "--verbose=4", appPath],
    "The app code signature cannot be inspected.",
  );
  const signature = parseCodeSignatureDetails(details, expectedIdentity);
  const version = commandText(
    "/usr/bin/plutil",
    [
      "-extract",
      "CFBundleShortVersionString",
      "raw",
      "-o",
      "-",
      join(appPath, "Contents", "Info.plist"),
    ],
    "The app version cannot be inspected.",
  );
  if (version !== expectedVersion) {
    throw new Error("The signed app version does not match the release tag.");
  }
  const architectures = commandText(
    "/usr/bin/lipo",
    ["-archs", join(appPath, "Contents", "MacOS", "TouchGrassBar")],
    "The app architecture cannot be inspected.",
  );
  if (architectures !== "arm64") {
    throw new Error("The release app is not Apple-silicon-only.");
  }
  commandPasses(
    "/usr/bin/xcrun",
    ["stapler", "validate", appPath],
    "The app stapling result is invalid.",
  );
  commandPasses(
    "/usr/sbin/spctl",
    ["--assess", "--type", "execute", appPath],
    "The app Gatekeeper assessment failed.",
  );
  return {
    ...signature,
    certificateSha256: extractCertificateSha256(appPath, temporaryDirectory),
  };
}

function notarizeDmg(dmgPath: string) {
  const result = spawnSync(
    "/usr/bin/xcrun",
    [
      "notarytool",
      "submit",
      dmgPath,
      "--key",
      requiredEnvironment("APPLE_API_KEY_PATH"),
      "--key-id",
      requiredEnvironment("APPLE_API_KEY"),
      "--issuer",
      requiredEnvironment("APPLE_API_ISSUER"),
      "--wait",
      "--output-format",
      "json",
    ],
    { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] },
  );
  if (result.status !== 0) {
    throw new Error("The independent DMG notarization request failed.");
  }
  let response: { status?: unknown };
  try {
    response = JSON.parse(result.stdout) as { status?: unknown };
  } catch {
    throw new Error("The independent DMG notarization result is unreadable.");
  }
  if (response.status !== "Accepted") {
    throw new Error("The independent DMG notarization result was not accepted.");
  }
}

function verifyDmg(dmgPath: string, expectedIdentity: string) {
  commandPasses(
    "/usr/bin/codesign",
    ["--verify", "--strict", dmgPath],
    "The DMG code signature is invalid.",
  );
  const details = commandText(
    "/usr/bin/codesign",
    ["--display", "--verbose=4", dmgPath],
    "The DMG code signature cannot be inspected.",
  );
  const identity = /^Authority=(.+)$/mu.exec(details)?.[1];
  if (identity !== expectedIdentity || !/^Timestamp=.+$/mu.test(details)) {
    throw new Error("The DMG signing identity or timestamp is invalid.");
  }
  commandPasses(
    "/usr/bin/xcrun",
    ["stapler", "staple", dmgPath],
    "The DMG ticket cannot be stapled.",
  );
  commandPasses(
    "/usr/bin/xcrun",
    ["stapler", "validate", dmgPath],
    "The DMG stapling result is invalid.",
  );
  commandPasses(
    "/usr/sbin/spctl",
    [
      "--assess",
      "--type",
      "open",
      "--context",
      "context:primary-signature",
      dmgPath,
    ],
    "The DMG Gatekeeper assessment failed.",
  );
}

function verifyUpdaterSignature(archivePath: string, signaturePath: string) {
  commandPasses(
    "cargo",
    [
      "run",
      "--quiet",
      "--manifest-path",
      join(tauriRoot, "Cargo.toml"),
      "--bin",
      "verify_update_signature",
      "--",
      archivePath,
      signaturePath,
      tauriConfigPath,
    ],
    "The Tauri updater signature is invalid.",
  );
}

function assertSameApp(
  expected: ReturnType<typeof verifyApp>,
  actual: ReturnType<typeof verifyApp>,
) {
  if (
    actual.cdhash !== expected.cdhash ||
    actual.certificateSha256 !== expected.certificateSha256 ||
    actual.identity !== expected.identity ||
    actual.teamIdentifier !== expected.teamIdentifier
  ) {
    throw new Error("A packaged app does not match the trusted release app.");
  }
}

function verifyArchiveApp(
  archivePath: string,
  expectedApp: ReturnType<typeof verifyApp>,
  expectedIdentity: string,
  expectedVersion: string,
  temporaryDirectory: string,
) {
  const entries = commandText(
    "/usr/bin/tar",
    ["-tzf", archivePath],
    "The updater archive cannot be inspected.",
  ).split("\n");
  validateUpdaterArchiveEntries(entries);
  const extractionDirectory = join(temporaryDirectory, "updater");
  mkdirSync(extractionDirectory, { recursive: true });
  commandPasses(
    "/usr/bin/tar",
    ["-xzf", archivePath, "-C", extractionDirectory],
    "The updater archive cannot be extracted.",
  );
  const extractedApp = verifyApp(
    join(extractionDirectory, "TouchGrassBar.app"),
    expectedIdentity,
    expectedVersion,
    temporaryDirectory,
  );
  assertSameApp(expectedApp, extractedApp);
}

function verifyDmgApp(
  dmgPath: string,
  expectedApp: ReturnType<typeof verifyApp>,
  expectedIdentity: string,
  expectedVersion: string,
  temporaryDirectory: string,
) {
  const mountPoint = join(temporaryDirectory, "dmg");
  mkdirSync(mountPoint, { recursive: true });
  commandPasses(
    "/usr/bin/hdiutil",
    ["attach", dmgPath, "-nobrowse", "-readonly", "-mountpoint", mountPoint],
    "The release DMG cannot be mounted.",
  );
  let verificationError: unknown;
  try {
    const mountedApp = verifyApp(
      join(mountPoint, "TouchGrassBar.app"),
      expectedIdentity,
      expectedVersion,
      temporaryDirectory,
    );
    assertSameApp(expectedApp, mountedApp);
  } catch (error) {
    verificationError = error;
  }
  const detached = spawnSync(
    "/usr/bin/hdiutil",
    ["detach", mountPoint],
    { stdio: "ignore" },
  );
  if (detached.status !== 0) {
    throw new Error("The release DMG cleanup failed.");
  }
  if (verificationError) throw verificationError;
}

function workflowActionVersions() {
  const actionPattern = /^\s*-?\s*uses:\s*([^@\s]+)@([^\s#]+)(?:\s+#.*)?$/gmu;
  const versions = new Map<string, string>();
  for (const workflowName of ["ci.yml", "release.yml"]) {
    const workflow = readFileSync(
      join(workspaceRoot, ".github", "workflows", workflowName),
      "utf8",
    );
    for (const match of workflow.matchAll(actionPattern)) {
      const action = match[1];
      const ref = match[2];
      if (!action || !ref || !/^(?:stable|v[0-9]+(?:\.[0-9]+){0,2})$/.test(ref)) {
        throw new Error("Every release-relevant Action must use a stable version.");
      }
      const existing = versions.get(action);
      if (existing && existing !== ref) {
        throw new Error("One Action has inconsistent versions.");
      }
      versions.set(action, ref);
    }
  }
  return [...versions.entries()]
    .sort(([left], [right]) => left.localeCompare(right))
    .map(([action, ref]) => ({ action, ref }));
}

function toolchainVersions() {
  return {
    bun: commandText("bun", ["--version"], "The Bun version is unavailable."),
    cargo: commandText(
      "cargo",
      ["--version"],
      "The Cargo version is unavailable.",
    ),
    notarytool: commandText(
      "/usr/bin/xcrun",
      ["notarytool", "--version"],
      "The notarytool version is unavailable.",
    ),
    rustc: commandText(
      "rustc",
      ["--version"],
      "The Rust version is unavailable.",
    ),
    xcode: commandText(
      "/usr/bin/xcodebuild",
      ["-version"],
      "The Xcode version is unavailable.",
    ).replaceAll("\n", "; "),
  };
}

function releaseNotes(tag: string, records: ArtifactRecord[]) {
  return `Signed and notarized TouchGrassBar draft candidate ${tag}.

This Release is a draft. Do not publish it until the exact candidate reaches GO_TO_PUBLISH and the public-release environment receives approval.

Sanitized trust checks:

- Developer ID signature, hardened runtime, and timestamp: PASS
- App and DMG notarization and stapling: PASS
- App and DMG Gatekeeper assessment: PASS
- Tauri updater signature: PASS
- Apple-silicon artifact binding: PASS

Candidate assets:

${records.map((record) => `- ${record.name}: ${record.bytes} bytes; SHA-256 ${record.sha256}`).join("\n")}
`;
}

function finalizeRelease() {
  const tag = requiredEnvironment("RELEASE_TAG");
  const commit = requiredEnvironment("RELEASE_COMMIT");
  const workflowRunId = requiredEnvironment("GITHUB_RUN_ID");
  const mainCiRunId = requiredEnvironment("RELEASE_MAIN_CI_RUN_ID");
  const expectedIdentity = requiredEnvironment("APPLE_SIGNING_IDENTITY");
  const { version } = parseStableReleaseTag(tag);
  if (
    process.platform !== "darwin" ||
    process.arch !== "arm64" ||
    Bun.env.CI !== "true" ||
    Bun.env.GITHUB_ACTIONS !== "true" ||
    Bun.env.GITHUB_REF_TYPE !== "tag" ||
    Bun.env.GITHUB_SHA !== commit ||
    !Bun.env.GITHUB_WORKFLOW_REF?.split("@", 1)[0]?.endsWith(
      "/.github/workflows/release.yml",
    )
  ) {
    throw new Error("Release finalization is restricted to the tagged arm64 workflow.");
  }

  const names = releaseAssetNames(tag);
  const appPath = join(bundleRoot, "macos", "TouchGrassBar.app");
  const dmgPath = join(bundleRoot, "dmg", names.dmg);
  const rawUpdaterArchivePath = join(
    bundleRoot,
    "macos",
    "TouchGrassBar.app.tar.gz",
  );
  const rawUpdaterSignaturePath = `${rawUpdaterArchivePath}.sig`;
  const configurationPath = join(
    outputDirectory,
    "release-configuration.json",
  );
  for (const expectedPath of [
    appPath,
    dmgPath,
    rawUpdaterArchivePath,
    rawUpdaterSignaturePath,
    configurationPath,
  ]) {
    if (!existsSync(expectedPath)) {
      throw new Error("A required release artifact is absent.");
    }
  }

  const temporaryDirectory = mkdtempSync(join(tmpdir(), "touchgrass-release."));
  try {
    const trustedApp = verifyApp(
      appPath,
      expectedIdentity,
      version,
      temporaryDirectory,
    );
    verifyUpdaterSignature(rawUpdaterArchivePath, rawUpdaterSignaturePath);
    verifyArchiveApp(
      rawUpdaterArchivePath,
      trustedApp,
      expectedIdentity,
      version,
      temporaryDirectory,
    );
    notarizeDmg(dmgPath);
    verifyDmg(dmgPath, expectedIdentity);
    verifyDmgApp(
      dmgPath,
      trustedApp,
      expectedIdentity,
      version,
      temporaryDirectory,
    );

    const updaterArchivePath = join(outputDirectory, names.updaterArchive);
    const updaterSignaturePath = join(outputDirectory, names.updaterSignature);
    const outputDmgPath = join(outputDirectory, names.dmg);
    copyFileSync(rawUpdaterArchivePath, updaterArchivePath);
    copyFileSync(rawUpdaterSignaturePath, updaterSignaturePath);
    copyFileSync(dmgPath, outputDmgPath);
    const capturedAt = new Date().toISOString();
    const signature = readFileSync(updaterSignaturePath, "utf8").trim();
    const latestManifest = createLatestManifest({
      notes: `TouchGrassBar ${version}`,
      pubDate: capturedAt,
      signature,
      tag,
      updaterArchiveName: names.updaterArchive,
    });
    const latestPath = join(outputDirectory, names.latest);
    writeFileSync(latestPath, `${JSON.stringify(latestManifest, null, 2)}\n`, {
      encoding: "utf8",
      mode: 0o644,
    });
    const primaryArtifacts = [
      outputDmgPath,
      updaterArchivePath,
      updaterSignaturePath,
      latestPath,
    ].map(artifactRecord);
    const checksumsPath = join(outputDirectory, names.checksums);
    writeFileSync(checksumsPath, formatChecksums(primaryArtifacts), {
      encoding: "utf8",
      mode: 0o644,
    });
    const trustArtifacts = [...primaryArtifacts, artifactRecord(checksumsPath)];
    const configuration = JSON.parse(
      readFileSync(configurationPath, "utf8"),
    ) as unknown;
    const governancePath = join(
      workspaceRoot,
      ".github",
      "release-governance.json",
    );
    const governance = JSON.parse(readFileSync(governancePath, "utf8")) as unknown;
    const receipt = createTrustReceipt({
      actions: workflowActionVersions(),
      artifacts: trustArtifacts,
      capturedAt,
      certificateSha256: trustedApp.certificateSha256,
      commit,
      configuration,
      governance,
      governanceSha256: sha256File(governancePath),
      identity: trustedApp.identity,
      mainCiRunId,
      tag,
      teamIdentifier: trustedApp.teamIdentifier,
      toolchains: toolchainVersions(),
      workflowRunId,
    });
    writeFileSync(
      join(outputDirectory, names.receipt),
      `${JSON.stringify(receipt, null, 2)}\n`,
      { encoding: "utf8", mode: 0o644 },
    );
    writeFileSync(
      join(outputDirectory, "release-notes.md"),
      releaseNotes(tag, trustArtifacts),
      { encoding: "utf8", mode: 0o644 },
    );
    rmSync(configurationPath, { force: true });
    console.log(
      `Release trust: PASS (${trustArtifacts.length} checked artifacts, arm64).`,
    );
  } finally {
    rmSync(temporaryDirectory, { force: true, recursive: true });
  }
}

if (import.meta.main) finalizeRelease();

export {
  createTrustReceipt,
  formatChecksums,
  parseCodeSignatureDetails,
  validateUpdaterArchiveEntries,
};
