#!/usr/bin/env bun

import { randomBytes } from "node:crypto";
import { chmodSync, existsSync, readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";

import {
  developmentTarget,
  localEnvironmentPath,
  readLocalDevelopmentEnvironment,
  workspaceRoot,
} from "./development-environment";

const convexCli = resolve(
  workspaceRoot,
  "packages/backend/node_modules/convex/bin/main.js",
);
const waitTimeoutMs = 60_000;

type CommandResult = {
  exitCode: number;
  stdout: string;
};

function convexArguments(argumentsList: string[]) {
  return [process.execPath, convexCli, ...argumentsList];
}

function anonymousLocalEnvironment() {
  const environment = { ...process.env };
  for (const name of [
    "CONVEX_DEPLOYMENT",
    "CONVEX_DEPLOY_KEY",
    "CONVEX_DEPLOYMENT_TOKEN",
    "CONVEX_SELF_HOSTED_ADMIN_KEY",
    "CONVEX_SELF_HOSTED_URL",
    "CONVEX_SITE_URL",
    "CONVEX_URL",
  ]) {
    delete environment[name];
  }
  environment.CONVEX_AGENT_MODE = "anonymous";
  return environment;
}

async function runInherited(
  executable: string,
  argumentsList: string[],
  environment: Record<string, string | undefined> = process.env,
) {
  const child = Bun.spawn([executable, ...argumentsList], {
    cwd: workspaceRoot,
    env: environment,
    stdin: "inherit",
    stderr: "inherit",
    stdout: "inherit",
  });
  return child.exited;
}

async function runCaptured(
  argumentsList: string[],
  options: {
    environment?: Record<string, string | undefined>;
    input?: string;
  } = {},
): Promise<CommandResult> {
  const child = Bun.spawn(convexArguments(argumentsList), {
    cwd: workspaceRoot,
    env: options.environment ?? process.env,
    stdin: options.input === undefined ? "ignore" : "pipe",
    stderr: "pipe",
    stdout: "pipe",
  });
  if (options.input !== undefined) {
    child.stdin.write(options.input);
    child.stdin.end();
  }
  const stdout = new Response(child.stdout).text();
  const stderr = new Response(child.stderr).text();
  const exitCode = await child.exited;
  await stderr;
  return { exitCode, stdout: await stdout };
}

async function installDependencies() {
  const exitCode = await runInherited("bun", ["install", "--frozen-lockfile"]);
  if (exitCode !== 0) throw new Error("Workspace dependency setup failed.");
}

function removeObsoleteEnvironmentAliases() {
  if (!existsSync(localEnvironmentPath)) return;
  const obsoleteNames = new Set([
    "TOUCHGRASS_AUTH_SITE_URL",
    "TOUCHGRASS_CONVEX_URL",
  ]);
  const current = readFileSync(localEnvironmentPath, "utf8");
  const next = current
    .split(/\r?\n/)
    .filter((line) => {
      const name = /^\s*(?:export\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*=/.exec(
        line,
      )?.[1];
      return !name || !obsoleteNames.has(name);
    })
    .join("\n")
    .replace(/^\n+|\n+$/g, "");
  const normalized = next ? `${next}\n` : "";
  if (normalized === current) return;
  writeFileSync(localEnvironmentPath, normalized, { mode: 0o600 });
  chmodSync(localEnvironmentPath, 0o600);
}

async function provisionLocalDeployment() {
  const result = await runCaptured(
    ["dev", "--once", "--tail-logs", "disable"],
    { environment: anonymousLocalEnvironment() },
  );
  if (result.exitCode !== 0 || !existsSync(localEnvironmentPath)) {
    throw new Error("Local Convex setup failed.");
  }
}

async function waitForLocalBackend(
  backend: ReturnType<typeof Bun.spawn>,
) {
  const startedAt = Date.now();
  while (Date.now() - startedAt < waitTimeoutMs) {
    const result = await runCaptured(["env", "list", "--names-only"]);
    if (result.exitCode === 0) return result.stdout;
    if (backend.exitCode !== null) {
      throw new Error("The local Convex backend stopped during setup.");
    }
    await Bun.sleep(500);
  }
  throw new Error("The local Convex backend did not become ready.");
}

async function ensureLocalBetterAuthSecret() {
  const backend = Bun.spawn(
    convexArguments([
      "dev",
      "--codegen",
      "disable",
      "--tail-logs",
      "disable",
      "--typecheck",
      "disable",
    ]),
    {
      cwd: workspaceRoot,
      env: process.env,
      stdin: "ignore",
      stderr: "pipe",
      stdout: "pipe",
    },
  );
  const stdout = new Response(backend.stdout).text();
  const stderr = new Response(backend.stderr).text();
  try {
    const names = await waitForLocalBackend(backend);
    if (!names.split(/\r?\n/).includes("BETTER_AUTH_SECRET")) {
      const secret = randomBytes(48).toString("base64url");
      const result = await runCaptured(
        ["env", "set", "BETTER_AUTH_SECRET"],
        { input: `${secret}\n` },
      );
      if (result.exitCode !== 0) {
        throw new Error("The local Better Auth secret could not be set.");
      }
    }
  } finally {
    backend.kill();
    await backend.exited;
    await stdout;
    await stderr;
  }
}

async function pushSelectedBackend() {
  const result = await runCaptured([
    "dev",
    "--once",
    "--tail-logs",
    "disable",
  ]);
  if (result.exitCode !== 0) {
    throw new Error("The selected Convex backend could not be prepared.");
  }
}

if (process.argv.length > 2) {
  throw new Error(`Unknown argument(s): ${process.argv.slice(2).join(", ")}`);
}

await installDependencies();
removeObsoleteEnvironmentAliases();
let environment = readLocalDevelopmentEnvironment();
if (!environment.CONVEX_DEPLOYMENT?.trim()) {
  await provisionLocalDeployment();
  environment = readLocalDevelopmentEnvironment();
}

const target = developmentTarget(environment);
Object.assign(process.env, environment);
if (target === "local") await ensureLocalBetterAuthSecret();
await pushSelectedBackend();

console.log(`Setup complete. Convex target: ${target}.`);
console.log("Run `bun env:check`, then `bun dev`.");
