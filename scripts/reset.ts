#!/usr/bin/env bun

import { execFileSync, spawnSync } from "node:child_process";
import {
  chmodSync,
  existsSync,
  readFileSync,
  rmSync,
  unlinkSync,
  writeFileSync,
} from "node:fs";
import { homedir } from "node:os";
import { join, resolve } from "node:path";
import { createInterface } from "node:readline/promises";

import { resolveDevInstance } from "../apps/desktop/src/dev/dev-instance";
import {
  localEnvironmentPath,
  workspaceRoot,
} from "./development-environment";
import {
  activeDevelopmentRunnerProcessId,
  developmentRunnerPath,
  processIsRunning,
} from "./development-runner-lease";

type CleanupTarget = {
  description: string;
  path: string;
};

const argumentsSet = new Set(process.argv.slice(2));
const supportedArguments = new Set([
  "--dry-run",
  "--production",
]);
const unknownArguments = [...argumentsSet].filter(
  (argument) => !supportedArguments.has(argument),
);
if (unknownArguments.length > 0) {
  throw new Error(`Unknown argument(s): ${unknownArguments.join(", ")}`);
}

const dryRun = argumentsSet.has("--dry-run");
const production = argumentsSet.has("--production");
if (process.platform !== "darwin") {
  throw new Error("TouchGrassBar reset currently supports macOS only.");
}

const userHome = homedir();
if (!userHome || userHome === "/") {
  throw new Error("A safe user home directory could not be resolved.");
}

function gitText(argumentsList: string[]) {
  return execFileSync("git", argumentsList, {
    cwd: workspaceRoot,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "ignore"],
  }).trim();
}

function developmentNamespace() {
  const branch = gitText(["branch", "--show-current"]);
  const namespace = resolveDevInstance({
    branch: branch || `detached-${gitText(["rev-parse", "--short", "HEAD"])}`,
    worktreeSeed: gitText(["rev-parse", "--show-toplevel"]),
  }).namespace;
  if (!/^app\.touchgrass\.bar\.dev\.w[a-z0-9]+$/.test(namespace)) {
    throw new Error("The development namespace is invalid.");
  }
  return namespace;
}

function desktopTargets(identifier: string): CleanupTarget[] {
  return [
    {
      description: "desktop application state",
      path: join(userHome, "Library", "Application Support", identifier),
    },
    {
      description: "desktop cache",
      path: join(userHome, "Library", "Caches", identifier),
    },
    {
      description: "desktop WebKit data",
      path: join(userHome, "Library", "WebKit", identifier),
    },
    {
      description: "desktop HTTP storage",
      path: join(userHome, "Library", "HTTPStorages", identifier),
    },
    {
      description: "desktop cookies",
      path: join(userHome, "Library", "Cookies", `${identifier}.binarycookies`),
    },
    {
      description: "desktop preferences",
      path: join(userHome, "Library", "Preferences", `${identifier}.plist`),
    },
    {
      description: "desktop saved window state",
      path: join(
        userHome,
        "Library",
        "Saved Application State",
        `${identifier}.savedState`,
      ),
    },
  ];
}

async function stopDevelopmentRunner() {
  const processId = activeDevelopmentRunnerProcessId();
  if (processId === null) return;
  if (dryRun) {
    console.log("[dry-run] active development command");
    return;
  }
  process.kill(processId, "SIGTERM");
  const startedAt = Date.now();
  while (processIsRunning(processId) && Date.now() - startedAt < 5_000) {
    await Bun.sleep(100);
  }
  if (processIsRunning(processId)) {
    throw new Error("The active development command did not stop.");
  }
  if (existsSync(developmentRunnerPath)) unlinkSync(developmentRunnerPath);
}

function productionProcessIds() {
  const output = execFileSync("/bin/ps", ["-axo", "pid=,command="], {
    encoding: "utf8",
  });
  const productionPaths = [
    "/Applications/TouchGrassBar.app/Contents/MacOS/touchgrassbar",
    join(
      userHome,
      "Applications",
      "TouchGrassBar.app",
      "Contents",
      "MacOS",
      "touchgrassbar",
    ),
    join(
      workspaceRoot,
      "apps",
      "desktop",
      "src-tauri",
      "target",
      "release",
      "bundle",
      "macos",
      "TouchGrassBar.app",
      "Contents",
      "MacOS",
      "touchgrassbar",
    ),
  ];
  return output.split(/\r?\n/).flatMap((line) => {
    const match = /^\s*(\d+)\s+(.+)$/.exec(line);
    if (!match || !productionPaths.some((path) => match[2]!.includes(path))) {
      return [];
    }
    return [Number(match[1])];
  });
}

async function stopProductionApp() {
  const processIds = productionProcessIds();
  if (dryRun && processIds.length > 0) {
    console.log("[dry-run] running production app");
    return;
  }
  for (const processId of processIds) process.kill(processId, "SIGTERM");
  const startedAt = Date.now();
  while (
    processIds.some(processIsRunning) &&
    Date.now() - startedAt < 5_000
  ) {
    await Bun.sleep(100);
  }
  if (processIds.some(processIsRunning)) {
    throw new Error("The production app did not stop.");
  }
}

async function confirmReset() {
  if (dryRun) return;
  if (!process.stdin.isTTY || !process.stdout.isTTY) {
    throw new Error("Interactive confirmation is required for production reset.");
  }
  const prompt = createInterface({ input: process.stdin, output: process.stdout });
  try {
    const expected = "RESET PRODUCTION";
    const answer = await prompt.question(`Type ${expected} to continue: `);
    if (answer !== expected) throw new Error("Reset cancelled.");
  } finally {
    prompt.close();
  }
}

const keychainAccounts = [
  "recovery-key",
  "better-auth-session",
  "installation-credential",
  "signup-preparation",
] as const;

function resetKeychain(service: string) {
  for (const account of keychainAccounts) {
    if (dryRun) continue;
    const result = spawnSync(
      "/usr/bin/security",
      ["delete-generic-password", "-s", service, "-a", account],
      { stdio: "ignore" },
    );
    if (result.status !== 0 && result.status !== 44) {
      throw new Error("A TouchGrassBar Keychain item could not be removed.");
    }
  }
  console.log(
    dryRun
      ? "[dry-run] TouchGrassBar Keychain items"
      : "Removed TouchGrassBar Keychain items.",
  );
}

function removeTargets(targets: CleanupTarget[]) {
  const allowed = new Set(targets.map((target) => resolve(target.path)));
  for (const target of targets) {
    const path = resolve(target.path);
    if (!allowed.has(path)) {
      throw new Error("A reset target is outside the approved scope.");
    }
    if (!existsSync(path)) continue;
    if (dryRun) {
      console.log(`[dry-run] ${target.description}`);
    } else {
      rmSync(path, { force: true, recursive: true });
      console.log(`Removed ${target.description}.`);
    }
  }
}

function clearConvexSelection() {
  if (!existsSync(localEnvironmentPath) || dryRun) return;
  const managedNames = new Set([
    "CONVEX_DEPLOYMENT",
    "CONVEX_DEPLOY_KEY",
    "CONVEX_DEPLOYMENT_TOKEN",
    "CONVEX_SELF_HOSTED_ADMIN_KEY",
    "CONVEX_SELF_HOSTED_URL",
    "CONVEX_SITE_URL",
    "CONVEX_URL",
    "TOUCHGRASS_AUTH_SITE_URL",
    "TOUCHGRASS_CONVEX_URL",
  ]);
  const source = readFileSync(localEnvironmentPath, "utf8")
    .split(/\r?\n/)
    .filter((line) => {
      const name = /^\s*(?:export\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*=/.exec(
        line,
      )?.[1];
      return !name || !managedNames.has(name);
    })
    .join("\n")
    .replace(/^\n+|\n+$/g, "");
  writeFileSync(localEnvironmentPath, source ? `${source}\n` : "", {
    mode: 0o600,
  });
  chmodSync(localEnvironmentPath, 0o600);
  for (const name of managedNames) delete process.env[name];
}

async function runClean() {
  if (dryRun) {
    const result = Bun.spawnSync(["bun", "scripts/clean.ts", "--dry-run"], {
      cwd: workspaceRoot,
      env: process.env,
      stderr: "inherit",
      stdout: "inherit",
    });
    if (result.exitCode !== 0) throw new Error("Build cleanup failed.");
    return;
  }
  const result = Bun.spawnSync(["bun", "scripts/clean.ts"], {
    cwd: workspaceRoot,
    env: process.env,
    stderr: "inherit",
    stdout: "inherit",
  });
  if (result.exitCode !== 0) throw new Error("Build cleanup failed.");
}

async function prepareFreshLocalEnvironment() {
  if (dryRun) return;
  const child = Bun.spawn(["bun", "scripts/setup.ts"], {
    cwd: workspaceRoot,
    env: process.env,
    stdin: "inherit",
    stderr: "inherit",
    stdout: "inherit",
  });
  if ((await child.exited) !== 0) {
    throw new Error("Fresh local setup failed after reset.");
  }
}

if (production) {
  console.log("Reset target: production app data on this Mac.");
  console.log("Remote production Convex data will be preserved.");
  await confirmReset();
  await stopProductionApp();
  removeTargets(desktopTargets("app.touchgrass.bar"));
  resetKeychain("app.touchgrass.bar.profile");
  console.log(
    dryRun
      ? "Production reset dry run complete. No state was changed."
      : "Production app state on this Mac was reset.",
  );
} else {
  const namespace = developmentNamespace();
  console.log("Reset target: all development state in this worktree.");
  console.log("Remote Convex deployments and production state will be preserved.");
  await stopDevelopmentRunner();
  await runClean();
  removeTargets([
    ...desktopTargets(namespace),
    {
      description: "worktree-local Convex data",
      path: join(workspaceRoot, ".convex"),
    },
  ]);
  resetKeychain(namespace);
  clearConvexSelection();
  await prepareFreshLocalEnvironment();
  console.log(
    dryRun
      ? "Development reset dry run complete. No state was changed."
      : "Development state was reset and a fresh local setup is ready.",
  );
}
