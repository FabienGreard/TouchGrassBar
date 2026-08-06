import { execFileSync } from "node:child_process";
import {
  chmodSync,
  closeSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  openSync,
  readFileSync,
  unlinkSync,
  writeFileSync,
} from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

import {
  resolveDevInstance,
  type DevInstance,
} from "../apps/desktop/src/dev/dev-instance";
import {
  developmentEntitlements,
  signingTeamIdentifier,
} from "../apps/desktop/src/dev/dev-signing";
import { developmentTarget } from "./development-environment";
import { activeDevelopmentRunnerProcessId } from "./development-runner-lease";
import { resolveDevelopmentSigningConfiguration } from "./macos-development-signing";

const workspaceRoot = resolve(import.meta.dir, "..");
const desktopRoot = join(workspaceRoot, "apps", "desktop");
const generatedConfigDirectory = join(
  desktopRoot,
  "src-tauri",
  ".dev-instance",
);
const generatedConfigPath = join(generatedConfigDirectory, "tauri.conf.json");
const generatedEntitlementsPath = join(
  generatedConfigDirectory,
  "entitlements.plist",
);
const generatedInfoPlistPath = join(generatedConfigDirectory, "Info.plist");
const generatedConfigArgument = join(
  "src-tauri",
  ".dev-instance",
  "tauri.conf.json",
);
const portLockDirectory = join(tmpdir(), "touchgrassbar-dev-ports");
const signedRunnerPath = join(workspaceRoot, "scripts", "run-signed-macos-dev.ts");
const profileServiceEnvironmentNames = ["CONVEX_SITE_URL", "CONVEX_URL"] as const;

function requireProfileServiceEnvironment(
  environment: Record<string, string | undefined>,
) {
  const missing = profileServiceEnvironmentNames.filter(
    (name) => !environment[name]?.trim(),
  );
  if (missing.length === 0) return;
  throw new Error(
    `Desktop Profile services are not configured (${missing.join(", ")}). Run \`bun setup\` before \`bun dev\`.`,
  );
}

function gitText(argumentsList: string[]) {
  return execFileSync("git", argumentsList, {
    cwd: workspaceRoot,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "ignore"],
  }).trim();
}

function portIsAvailable(port: number) {
  return new Promise<boolean>((resolveAvailability) => {
    const server = createServer();
    server.unref();
    server.once("error", () => resolveAvailability(false));
    server.listen({ host: "127.0.0.1", port }, () => {
      server.close(() => resolveAvailability(true));
    });
  });
}

type PortLease = {
  port: number;
  release: () => void;
};

function processIsRunning(processId: number) {
  try {
    process.kill(processId, 0);
    return true;
  } catch (error) {
    return (error as NodeJS.ErrnoException).code !== "ESRCH";
  }
}

function requireNoDevelopmentRunner() {
  if (activeDevelopmentRunnerProcessId() !== null) {
    throw new Error("Stop the active `bun dev` command before building a bundle.");
  }
}

function activePortLease(lockPath: string) {
  try {
    const processId = Number(readFileSync(lockPath, "utf8"));
    return !Number.isInteger(processId) || processId <= 0
      ? true
      : processIsRunning(processId);
  } catch {
    return true;
  }
}

function claimPort(port: number): PortLease | null {
  mkdirSync(portLockDirectory, { recursive: true });
  const lockPath = join(portLockDirectory, `${port}.lock`);

  for (let attempt = 0; attempt < 2; attempt += 1) {
    try {
      const descriptor = openSync(lockPath, "wx", 0o600);
      try {
        writeFileSync(descriptor, String(process.pid), "utf8");
      } finally {
        closeSync(descriptor);
      }

      let released = false;
      return {
        port,
        release: () => {
          if (released) return;
          released = true;
          try {
            unlinkSync(lockPath);
          } catch {
            // The lease is already absent.
          }
        },
      };
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== "EEXIST") throw error;
      if (activePortLease(lockPath)) return null;
      try {
        unlinkSync(lockPath);
      } catch {
        return null;
      }
    }
  }
  return null;
}

async function availablePort(preferred: number) {
  for (let offset = 0; offset < 1_000; offset += 1) {
    const candidate = 15_000 + ((preferred - 15_000 + offset) % 1_000);
    const lease = claimPort(candidate);
    if (!lease) continue;
    if (await portIsAvailable(candidate)) return lease;
    lease.release();
  }
  throw new Error("No development port is available from 15000 through 15999.");
}

function devCsp(port: number) {
  const origin = `http://127.0.0.1:${port}`;
  return [
    `default-src 'self' ${origin}`,
    `connect-src ipc: http://ipc.localhost ${origin} ws://127.0.0.1:${port}`,
    `img-src 'self' data: asset: http://asset.localhost`,
    `style-src 'self' 'unsafe-inline' ${origin}`,
    `script-src 'self' 'unsafe-eval' ${origin}`,
  ].join("; ");
}

async function writeTauriConfig(instance: DevInstance, bundle: boolean) {
  mkdirSync(generatedConfigDirectory, { recursive: true });
  await Bun.write(
    generatedConfigPath,
    `${JSON.stringify(
      {
        app: { security: { devCsp: devCsp(instance.port) } },
        ...(bundle
          ? {
              bundle: {
                macOS: {
                  entitlements: ".dev-instance/entitlements.plist",
                },
                targets: ["app"],
              },
            }
          : {}),
        build: {
          beforeDevCommand: `bun run dev:web -- --port ${instance.port}`,
          devUrl: `http://127.0.0.1:${instance.port}`,
        },
        identifier: instance.bundleIdentifier,
        productName: instance.productName,
      },
      null,
      2,
    )}\n`,
  );
}

async function writeDevelopmentEntitlements(
  instance: DevInstance,
  signingIdentity: string,
) {
  await Bun.write(
    generatedEntitlementsPath,
    developmentEntitlements({
      bundleIdentifier: instance.bundleIdentifier,
      teamIdentifier: signingTeamIdentifier(signingIdentity),
    }),
  );
}

function escapedPlistValue(value: string) {
  return value
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&apos;");
}

async function writeDevelopmentInfoPlist(instance: DevInstance) {
  const bundleIdentifier = escapedPlistValue(instance.bundleIdentifier);
  const productName = escapedPlistValue(instance.productName);
  await Bun.write(
    generatedInfoPlistPath,
    `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key>
  <string>en</string>
  <key>CFBundleDisplayName</key>
  <string>${productName}</string>
  <key>CFBundleExecutable</key>
  <string>touchgrassbar</string>
  <key>CFBundleIdentifier</key>
  <string>${bundleIdentifier}</string>
  <key>CFBundleName</key>
  <string>${productName}</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>0.0.0</string>
  <key>CFBundleVersion</key>
  <string>1</string>
  <key>LSUIElement</key>
  <true/>
  <key>NSHighResolutionCapable</key>
  <true/>
</dict>
</plist>
`,
  );
}

function printInstance(instance: DevInstance, bundle: boolean) {
  console.log(`Development instance: ${instance.label}`);
  console.log(`Accent: ${instance.accent}`);
  if (bundle) {
    console.log("Surface: signed application bundle");
  } else {
    console.log(`Browser preview: http://127.0.0.1:${instance.port}`);
    console.log("Surface: native app and browser preview");
  }
}

function signDevelopmentBundle(
  appPath: string,
  signingIdentity: string,
  provisioningProfile: string,
) {
  const contentsPath = join(appPath, "Contents");
  const embeddedProfilePath = join(contentsPath, "embedded.provisionprofile");
  const helperPath = join(contentsPath, "MacOS", "export_native_contract");
  copyFileSync(provisioningProfile, embeddedProfilePath);
  chmodSync(embeddedProfilePath, 0o644);
  if (existsSync(helperPath)) {
    execFileSync(
      "/usr/bin/codesign",
      [
        "--force",
        "--options",
        "runtime",
        "--timestamp=none",
        "--sign",
        signingIdentity,
        helperPath,
      ],
      { stdio: "inherit" },
    );
  }
  execFileSync(
    "/usr/bin/codesign",
    [
      "--force",
      "--options",
      "runtime",
      "--timestamp=none",
      "--generate-entitlement-der",
      "--entitlements",
      generatedEntitlementsPath,
      "--sign",
      signingIdentity,
      appPath,
    ],
    { stdio: "inherit" },
  );
  execFileSync(
    "/usr/bin/codesign",
    ["--verify", "--deep", "--strict", appPath],
    { stdio: "inherit" },
  );
}

async function buildDevelopmentBundle(
  instance: DevInstance,
  environment: Record<string, string | undefined>,
  signingIdentity: string,
  provisioningProfile: string,
  openBundle: boolean,
) {
  const child = Bun.spawn(
    [
      "bun",
      "run",
      "tauri",
      "build",
      "--bundles",
      "app",
      "--config",
      generatedConfigArgument,
    ],
    {
      cwd: desktopRoot,
      env: environment,
      stdin: "inherit",
      stderr: "inherit",
      stdout: "inherit",
    },
  );
  if ((await child.exited) !== 0) {
    throw new Error("The development application bundle could not be built.");
  }
  const appPath = join(
    desktopRoot,
    "src-tauri",
    "target",
    "release",
    "bundle",
    "macos",
    `${instance.productName}.app`,
  );
  if (!existsSync(appPath)) {
    throw new Error("The development application bundle was not created.");
  }
  signDevelopmentBundle(appPath, signingIdentity, provisioningProfile);
  console.log("Development bundle signature and Keychain access: verified");
  if (!openBundle) return 0;
  const opened = Bun.spawn(["/usr/bin/open", "-n", "-W", appPath], {
    env: environment,
    stdin: "ignore",
    stderr: "inherit",
    stdout: "inherit",
  });
  return opened.exited;
}

function convexCommand(argumentsList: string[]) {
  return [
    "bun",
    "run",
    "--cwd",
    "packages/backend",
    "convex",
    ...argumentsList,
  ];
}

async function prepareBundleBackend(
  target: ReturnType<typeof developmentTarget>,
) {
  if (target === "cloud development") {
    const result = Bun.spawnSync(
      convexCommand(["dev", "--once", "--tail-logs", "disable"]),
      {
        cwd: workspaceRoot,
        env: process.env,
        stderr: "inherit",
        stdout: "inherit",
      },
    );
    if (result.exitCode !== 0) {
      throw new Error("The cloud development backend could not be prepared.");
    }
    return null;
  }

  const backend = Bun.spawn(convexCommand(["dev", "--tail-logs", "disable"]), {
    cwd: workspaceRoot,
    env: process.env,
    stdin: "ignore",
    stderr: "inherit",
    stdout: "inherit",
  });
  const startedAt = Date.now();
  while (Date.now() - startedAt < 60_000) {
    const probe = Bun.spawnSync(convexCommand(["env", "list", "--names-only"]), {
      cwd: workspaceRoot,
      env: process.env,
      stderr: "ignore",
      stdout: "ignore",
    });
    if (probe.exitCode === 0) return backend;
    if (backend.exitCode !== null) {
      throw new Error("The local Convex backend stopped before bundle launch.");
    }
    await Bun.sleep(500);
  }
  backend.kill();
  await backend.exited;
  throw new Error("The local Convex backend did not become ready.");
}

async function main() {
  const argumentsList = process.argv.slice(2);
  const argumentsSet = new Set(argumentsList);
  const bundle = argumentsSet.has("--bundle");
  const openBundle = !argumentsSet.has("--no-open");
  if (
    argumentsSet.size !== argumentsList.length ||
    [...argumentsSet].some(
      (argument) => argument !== "--bundle" && argument !== "--no-open",
    ) ||
    (argumentsSet.has("--no-open") && !bundle)
  ) {
    throw new Error(`Unknown argument(s): ${argumentsList.join(", ")}`);
  }
  requireProfileServiceEnvironment(Bun.env);
  const target = developmentTarget(Bun.env);
  if (bundle) requireNoDevelopmentRunner();
  const branch = gitText(["branch", "--show-current"]);
  const worktreeSeed = gitText(["rev-parse", "--show-toplevel"]);
  const requested = resolveDevInstance({
    accent: Bun.env.TOUCHGRASS_DEV_ACCENT,
    branch:
      branch || `detached-${gitText(["rev-parse", "--short", "HEAD"])}`,
    label: Bun.env.TOUCHGRASS_DEV_LABEL,
    worktreeSeed,
  });
  const portLease = bundle ? null : await availablePort(requested.port);
  if (portLease) process.once("exit", portLease.release);
  const instance = { ...requested, port: portLease?.port ?? requested.port };
  const { identity: signingIdentity, provisioningProfile } =
    resolveDevelopmentSigningConfiguration(
      instance.bundleIdentifier,
      Bun.env,
    );
  const serializedInstance = JSON.stringify(instance);
  const appBundlePath = join(
    generatedConfigDirectory,
    "TouchGrassBar Dev.app",
  );
  const environment = {
    ...Bun.env,
    CARGO_TARGET_AARCH64_APPLE_DARWIN_RUNNER: signedRunnerPath,
    TOUCHGRASS_DEV_APP_BUNDLE_PATH: appBundlePath,
    TOUCHGRASS_DEV_BUNDLE_IDENTIFIER: instance.bundleIdentifier,
    TOUCHGRASS_DEV_ENTITLEMENTS_PATH: generatedEntitlementsPath,
    TOUCHGRASS_DEV_INFO_PLIST_PATH: generatedInfoPlistPath,
    TOUCHGRASS_DEV_INSTANCE_LABEL: instance.label,
    TOUCHGRASS_DEV_INSTANCE_TAG: instance.tag,
    TOUCHGRASS_DEV_KEYCHAIN_SERVICE: instance.namespace,
    TOUCHGRASS_DEV_NAMESPACE: instance.namespace,
    TOUCHGRASS_DEV_PROVISIONING_PROFILE: provisioningProfile,
    TOUCHGRASS_DEV_SIGNING_IDENTITY: signingIdentity,
    VITE_TOUCHGRASS_DEV_INSTANCE: serializedInstance,
  };

  try {
    printInstance(instance, bundle);
    console.log(`Convex target: ${target}`);
    await writeTauriConfig(instance, bundle);
    await writeDevelopmentEntitlements(instance, signingIdentity);
    await writeDevelopmentInfoPlist(instance);
    if (bundle) {
      const backend = openBundle ? await prepareBundleBackend(target) : null;
      try {
        process.exitCode = await buildDevelopmentBundle(
          instance,
          environment,
          signingIdentity,
          provisioningProfile,
          openBundle,
        );
      } finally {
        if (backend?.exitCode === null) {
          backend.kill();
          await backend.exited;
        }
      }
      return;
    }
    const child = Bun.spawn(
      ["bun", "run", "tauri", "dev", "--config", generatedConfigArgument],
      {
        cwd: desktopRoot,
        env: environment,
        stderr: "inherit",
        stdout: "inherit",
      },
    );
    process.exitCode = await child.exited;
  } finally {
    portLease?.release();
  }
}

await main();
