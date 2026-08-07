#!/usr/bin/env bun

import { execFileSync, spawnSync } from "node:child_process";
import {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { homedir, tmpdir } from "node:os";
import { join, resolve } from "node:path";

import {
  developmentEntitlements,
  parseCodeSigningIdentities,
  signingTeamIdentifier,
} from "../apps/desktop/src/dev/dev-signing";
import { parseStableReleaseTag } from "./release-contract";

const bundleIdentifier = "app.touchgrass.bar";
const productName = "TouchGrassBar";
const workspaceRoot = resolve(import.meta.dir, "..");
const desktopRoot = join(workspaceRoot, "apps", "desktop");
const tauriRoot = join(desktopRoot, "src-tauri");
const generatedDirectory = join(tauriRoot, ".dev-instance");
const profilePath = resolve(
  Bun.env.TOUCHGRASS_PROD_PROVISIONING_PROFILE?.trim() ||
    join(generatedDirectory, "prod.provisionprofile"),
);
const entitlementsPath = join(generatedDirectory, "prod-entitlements.plist");
const configPath = join(generatedDirectory, "prod-tauri.conf.json");
const configArgument = join(
  "src-tauri",
  ".dev-instance",
  "prod-tauri.conf.json",
);
const appPath = join(
  tauriRoot,
  "target",
  "release",
  "bundle",
  "macos",
  `${productName}.app`,
);
const profileEnvironmentNames = ["CONVEX_SITE_URL", "CONVEX_URL"] as const;
const developmentEnvironmentNames = [
  "TOUCHGRASS_DEV_APP_BUNDLE_PATH",
  "TOUCHGRASS_DEV_BUNDLE_IDENTIFIER",
  "TOUCHGRASS_DEV_ENTITLEMENTS_PATH",
  "TOUCHGRASS_DEV_INFO_PLIST_PATH",
  "TOUCHGRASS_DEV_INSTANCE_LABEL",
  "TOUCHGRASS_DEV_INSTANCE_TAG",
  "TOUCHGRASS_DEV_KEYCHAIN_SERVICE",
  "TOUCHGRASS_DEV_NAMESPACE",
  "TOUCHGRASS_DEV_PROVISIONING_PROFILE",
  "TOUCHGRASS_DEV_SIGNING_IDENTITY",
  "VITE_TOUCHGRASS_DEV_INSTANCE",
] as const;

function commandOutput(
  executable: string,
  argumentsList: string[],
  input?: string | Uint8Array,
) {
  return execFileSync(executable, argumentsList, {
    encoding: "utf8",
    input,
    stdio: [input === undefined ? "ignore" : "pipe", "pipe", "pipe"],
  }).trim();
}

function plistValue(path: string, keyPath: string) {
  return commandOutput("/usr/bin/plutil", [
    "-extract",
    keyPath,
    "raw",
    "-o",
    "-",
    path,
  ]);
}

function requireProfileEnvironment() {
  const missing = profileEnvironmentNames.filter(
    (name) => !Bun.env[name]?.trim(),
  );
  if (missing.length > 0) {
    throw new Error(
      `Production Profile services are not configured (${missing.join(", ")}).`,
    );
  }
}

function resolveProductionIdentity() {
  const identities = parseCodeSigningIdentities(
    commandOutput("/usr/bin/security", [
      "find-identity",
      "-v",
      "-p",
      "codesigning",
    ]),
  ).filter((identity) => identity.startsWith("Developer ID Application:"));
  const configured = Bun.env.TOUCHGRASS_PROD_SIGNING_IDENTITY?.trim();
  const configuredIdentity =
    configured || Bun.env.APPLE_SIGNING_IDENTITY?.trim();
  if (configuredIdentity) {
    if (!identities.includes(configuredIdentity)) {
      throw new Error("The configured production signing identity is not valid.");
    }
    return configuredIdentity;
  }
  if (identities.length === 1) return identities[0]!;
  if (identities.length === 0) {
    throw new Error("No valid Developer ID Application identity is installed.");
  }
  throw new Error(
    "Set TOUCHGRASS_PROD_SIGNING_IDENTITY because multiple production identities are installed.",
  );
}

function certificateFingerprint(path: string, format: "DER" | "PEM") {
  return commandOutput("/usr/bin/openssl", [
    "x509",
    "-inform",
    format,
    "-in",
    path,
    "-noout",
    "-fingerprint",
    "-sha1",
  ])
    .split("=")[1]
    ?.replaceAll(":", "");
}

function verifyProfile(signingIdentity: string, temporaryDirectory: string) {
  if (!existsSync(profilePath)) {
    throw new Error("The production provisioning profile is not installed.");
  }
  const decodedProfilePath = join(temporaryDirectory, "profile.plist");
  const profileEntitlementsPath = join(
    temporaryDirectory,
    "profile-entitlements.plist",
  );
  const profileCertificateBase64Path = join(
    temporaryDirectory,
    "profile-certificate.base64",
  );
  const profileCertificatePath = join(
    temporaryDirectory,
    "profile-certificate.cer",
  );
  const installedCertificatePath = join(
    temporaryDirectory,
    "installed-certificate.pem",
  );
  execFileSync(
    "/usr/bin/openssl",
    [
      "smime",
      "-verify",
      "-inform",
      "der",
      "-in",
      profilePath,
      "-noverify",
      "-out",
      decodedProfilePath,
    ],
    { stdio: "ignore" },
  );
  execFileSync(
    "/usr/bin/plutil",
    [
      "-extract",
      "Entitlements",
      "xml1",
      "-o",
      profileEntitlementsPath,
      decodedProfilePath,
    ],
    { stdio: "ignore" },
  );
  const teamIdentifier = signingTeamIdentifier(signingIdentity);
  const applicationIdentifier = `${teamIdentifier}.${bundleIdentifier}`;
  const keychainGroup = plistValue(
    profileEntitlementsPath,
    "keychain-access-groups.0",
  );
  if (
    plistValue(
      profileEntitlementsPath,
      "com\\.apple\\.application-identifier",
    ) !== applicationIdentifier ||
    plistValue(
      profileEntitlementsPath,
      "com\\.apple\\.developer\\.team-identifier",
    ) !== teamIdentifier ||
    (keychainGroup !== applicationIdentifier &&
      keychainGroup !== `${teamIdentifier}.*`) ||
    plistValue(decodedProfilePath, "Platform.0") !== "OSX" ||
    plistValue(decodedProfilePath, "ProvisionsAllDevices") !== "true"
  ) {
    throw new Error("The production provisioning profile binding is invalid.");
  }
  const expiration = Date.parse(
    plistValue(decodedProfilePath, "ExpirationDate"),
  );
  if (!Number.isFinite(expiration) || expiration <= Date.now()) {
    throw new Error("The production provisioning profile has expired.");
  }
  execFileSync(
    "/usr/bin/plutil",
    [
      "-extract",
      "DeveloperCertificates.0",
      "raw",
      "-o",
      profileCertificateBase64Path,
      decodedProfilePath,
    ],
    { stdio: "ignore" },
  );
  execFileSync(
    "/usr/bin/base64",
    [
      "-D",
      "-i",
      profileCertificateBase64Path,
      "-o",
      profileCertificatePath,
    ],
    { stdio: "ignore" },
  );
  const installedCertificate = commandOutput("/usr/bin/security", [
    "find-certificate",
    "-c",
    signingIdentity,
    "-p",
  ]);
  writeFileSync(installedCertificatePath, installedCertificate, "utf8");
  const profileFingerprint = certificateFingerprint(
    profileCertificatePath,
    "DER",
  );
  const installedFingerprint = certificateFingerprint(
    installedCertificatePath,
    "PEM",
  );
  if (!profileFingerprint || profileFingerprint !== installedFingerprint) {
    throw new Error(
      "The production profile does not contain the installed signing certificate.",
    );
  }
  return teamIdentifier;
}

async function writeProductionConfiguration(
  teamIdentifier: string,
  applicationOnly: boolean,
  version: string,
) {
  mkdirSync(generatedDirectory, { recursive: true });
  const embeddedProfileSource = join(
    generatedDirectory,
    "prod.provisionprofile",
  );
  if (profilePath !== embeddedProfileSource) {
    copyFileSync(profilePath, embeddedProfileSource);
  }
  chmodSync(embeddedProfileSource, 0o600);
  await Bun.write(
    entitlementsPath,
    developmentEntitlements({ bundleIdentifier, teamIdentifier }),
  );
  await Bun.write(
    configPath,
    `${JSON.stringify(
      {
        version,
        bundle: {
          createUpdaterArtifacts: true,
          macOS: {
            entitlements: ".dev-instance/prod-entitlements.plist",
            files: {
              "embedded.provisionprofile":
                ".dev-instance/prod.provisionprofile",
            },
          },
          ...(applicationOnly ? { targets: ["app"] } : {}),
        },
        identifier: bundleIdentifier,
        productName,
      },
      null,
      2,
    )}\n`,
  );
}

function productionBuildEnvironment() {
  const environment = { ...Bun.env };
  for (const name of developmentEnvironmentNames) delete environment[name];
  delete environment.APPLE_CERTIFICATE;
  delete environment.APPLE_CERTIFICATE_PASSWORD;
  delete environment.APPLE_SIGNING_IDENTITY;
  return environment;
}

async function buildApp() {
  console.log("Building the local production app bundle...");
  const child = Bun.spawn(
    [
      "bun",
      "run",
      "tauri",
      "build",
      "--bundles",
      "app",
      "--config",
      configArgument,
    ],
    {
      cwd: desktopRoot,
      env: productionBuildEnvironment(),
      stderr: "inherit",
      stdout: "inherit",
    },
  );
  const exitCode = await child.exited;
  if (exitCode !== 0) throw new Error("The production app build failed.");
  if (!existsSync(appPath)) {
    throw new Error("The production app bundle was not created.");
  }
}

function verifySignedApp(
  signingIdentity: string,
  temporaryDirectory: string,
  requireNotarization: boolean,
) {
  const contentsPath = join(appPath, "Contents");
  const embeddedProfilePath = join(
    contentsPath,
    "embedded.provisionprofile",
  );
  const signedEntitlementsPath = join(
    temporaryDirectory,
    "signed-entitlements.plist",
  );
  execFileSync(
    "/usr/bin/codesign",
    ["--verify", "--deep", "--strict", appPath],
    { stdio: "ignore" },
  );
  if (!readFileSync(profilePath).equals(readFileSync(embeddedProfilePath))) {
    throw new Error("The embedded production profile is not exact.");
  }
  const signatureDetails = spawnSync(
    "/usr/bin/codesign",
    ["--display", "--verbose=4", appPath],
    { encoding: "utf8" },
  );
  if (signatureDetails.status !== 0) {
    throw new Error("The production signature cannot be inspected.");
  }
  const details = `${signatureDetails.stdout}${signatureDetails.stderr}`;
  const actualBundleIdentifier = /^Identifier=(.+)$/mu.exec(details)?.[1];
  const actualTeamIdentifier = /^TeamIdentifier=(.+)$/mu.exec(details)?.[1];
  if (
    actualBundleIdentifier !== bundleIdentifier ||
    actualTeamIdentifier !== signingTeamIdentifier(signingIdentity) ||
    !details.includes("(runtime)")
  ) {
    throw new Error("The final production signature binding is invalid.");
  }
  const signedEntitlements = commandOutput("/usr/bin/codesign", [
    "--display",
    "--entitlements",
    "-",
    "--xml",
    appPath,
  ]);
  writeFileSync(signedEntitlementsPath, signedEntitlements, "utf8");
  const applicationIdentifier = `${actualTeamIdentifier}.${bundleIdentifier}`;
  if (
    plistValue(
      signedEntitlementsPath,
      "com\\.apple\\.application-identifier",
    ) !== applicationIdentifier ||
    plistValue(signedEntitlementsPath, "keychain-access-groups.0") !==
      applicationIdentifier
  ) {
    throw new Error("The final production Keychain entitlement is invalid.");
  }
  if (!requireNotarization) return;
  execFileSync("/usr/bin/xcrun", ["stapler", "validate", appPath], {
    stdio: "ignore",
  });
  execFileSync(
    "/usr/sbin/spctl",
    ["--assess", "--type", "execute", "--verbose=4", appPath],
    { stdio: "ignore" },
  );
}

function signApp(signingIdentity: string, temporaryDirectory: string) {
  const contentsPath = join(appPath, "Contents");
  const embeddedProfilePath = join(
    contentsPath,
    "embedded.provisionprofile",
  );
  const helperPath = join(contentsPath, "MacOS", "export_native_contract");
  copyFileSync(profilePath, embeddedProfilePath);
  chmodSync(embeddedProfilePath, 0o644);
  if (existsSync(helperPath)) {
    execFileSync(
      "/usr/bin/codesign",
      [
        "--force",
        "--options",
        "runtime",
        "--timestamp",
        "--sign",
        signingIdentity,
        helperPath,
      ],
      { stdio: "ignore" },
    );
  }
  execFileSync(
    "/usr/bin/codesign",
    [
      "--force",
      "--options",
      "runtime",
      "--timestamp",
      "--generate-entitlement-der",
      "--entitlements",
      entitlementsPath,
      "--sign",
      signingIdentity,
      appPath,
    ],
    { stdio: "ignore" },
  );
  verifySignedApp(signingIdentity, temporaryDirectory, false);
}

function appIsRunning() {
  return [productName, productName.toLowerCase()].some(
    (processName) =>
      spawnSync("/usr/bin/pgrep", ["-x", processName], {
        stdio: "ignore",
      }).status === 0,
  );
}

function notarizeAndValidate(temporaryDirectory: string) {
  const issuer = Bun.env.APPLE_API_ISSUER?.trim();
  const keyIdentifier = Bun.env.APPLE_API_KEY?.trim();
  const configuredKeyPath = Bun.env.APPLE_API_KEY_PATH?.trim();
  const keyPath = configuredKeyPath ||
    (keyIdentifier
      ? join(homedir(), "private_keys", `AuthKey_${keyIdentifier}.p8`)
      : undefined);
  if (!issuer || !keyIdentifier || !keyPath || !existsSync(keyPath)) {
    throw new Error("Production notarization credentials are incomplete.");
  }
  const archivePath = join(temporaryDirectory, "TouchGrassBar.zip");
  execFileSync(
    "/usr/bin/ditto",
    ["-c", "-k", "--keepParent", appPath, archivePath],
    { stdio: "ignore" },
  );
  execFileSync(
    "/usr/bin/xcrun",
    [
      "notarytool",
      "submit",
      archivePath,
      "--key",
      keyPath,
      "--key-id",
      keyIdentifier,
      "--issuer",
      issuer,
      "--wait",
    ],
    { stdio: "inherit" },
  );
  execFileSync("/usr/bin/xcrun", ["stapler", "staple", appPath], {
    stdio: "inherit",
  });
  execFileSync("/usr/bin/xcrun", ["stapler", "validate", appPath], {
    stdio: "inherit",
  });
  execFileSync(
    "/usr/sbin/spctl",
    ["--assess", "--type", "execute", "--verbose=4", appPath],
    { stdio: "inherit" },
  );
}

async function main() {
  const argumentsList = process.argv.slice(2);
  const mode = argumentsList[0] ?? "--release";
  if (
    argumentsList.length > 1 ||
    !new Set(["--prepare", "--release", "--verify"]).has(mode)
  ) {
    throw new Error(`Unknown argument(s): ${process.argv.slice(2).join(", ")}`);
  }
  const releaseWorkflow = Bun.env.GITHUB_WORKFLOW_REF
    ?.split("@", 1)[0]
    ?.endsWith("/.github/workflows/release.yml");
  if (
    Bun.env.CI !== "true" ||
    Bun.env.GITHUB_ACTIONS !== "true" ||
    Bun.env.GITHUB_REF_TYPE !== "tag" ||
    !releaseWorkflow
  ) {
    throw new Error(
      "desktop:release is available only in the tagged GitHub release workflow.",
    );
  }
  if (process.platform !== "darwin") {
    throw new Error("The local production app supports macOS only.");
  }
  const release = parseStableReleaseTag(Bun.env.GITHUB_REF_NAME?.trim() ?? "");
  if (mode === "--release" && appIsRunning()) {
    throw new Error("Quit every TouchGrassBar instance before running prod.");
  }
  requireProfileEnvironment();
  const signingIdentity = resolveProductionIdentity();
  const temporaryDirectory = mkdtempSync(
    join(tmpdir(), "touchgrass-prod-sign."),
  );
  try {
    const teamIdentifier = verifyProfile(
      signingIdentity,
      temporaryDirectory,
    );
    console.log("Production provisioning profile: verified");
    await writeProductionConfiguration(
      teamIdentifier,
      mode === "--release",
      release.version,
    );
    if (mode === "--prepare") {
      console.log("Production entitlement binding: prepared");
      return;
    }
    if (mode === "--verify") {
      verifySignedApp(signingIdentity, temporaryDirectory, true);
      console.log(
        "Production signature, Keychain access, notarization, and Gatekeeper assessment: verified",
      );
      return;
    }
    await buildApp();
    signApp(signingIdentity, temporaryDirectory);
    console.log("Production app signature and Keychain access: verified");
    notarizeAndValidate(temporaryDirectory);
    console.log("Production app notarization and Gatekeeper assessment: verified");
  } finally {
    rmSync(temporaryDirectory, { force: true, recursive: true });
  }
}

await main();
