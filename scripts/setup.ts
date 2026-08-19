#!/usr/bin/env bun

import { randomBytes } from "node:crypto";
import { chmodSync, existsSync, readFileSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { resolve } from "node:path";

import {
  developmentTarget,
  localEnvironmentPath,
  readLocalDevelopmentEnvironment,
  workspaceRoot,
} from "./development-environment";
import { convexCommandEnvironment } from "./convex-command-environment";

const convexCli = resolve(workspaceRoot, "packages/backend/node_modules/convex/bin/main.js");
type CommandResult = {
  exitCode: number;
  stderr: string;
  stdout: string;
};

function failureMessage(message: string, result: CommandResult) {
  const ansiEscape = new RegExp(`${String.fromCodePoint(27)}\\[[0-?]*[ -/]*[@-~]`, "g");
  const diagnostic = result.stderr
    .replace(ansiEscape, "")
    .replaceAll(workspaceRoot, "<workspace>")
    .replaceAll(homedir(), "<home>")
    .replace(/\b([A-Z][A-Z0-9_]*(?:SECRET|TOKEN|KEY))=\S+/g, "$1=<redacted>")
    .trim()
    .split(/\r?\n/)
    .slice(-8)
    .join("\n");
  return diagnostic ? `${message}\nConvex reported:\n${diagnostic}` : message;
}

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
    env:
      options.environment ??
      convexCommandEnvironment(argumentsList, process.env, readLocalDevelopmentEnvironment()),
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
  return {
    exitCode,
    stderr: await stderr,
    stdout: await stdout,
  };
}

async function installDependencies() {
  const exitCode = await runInherited("bun", ["install", "--frozen-lockfile"]);
  if (exitCode !== 0) throw new Error("Workspace dependency setup failed.");
}

function removeObsoleteEnvironmentAliases() {
  if (!existsSync(localEnvironmentPath)) return;
  const obsoleteNames = new Set(["TOUCHGRASS_AUTH_SITE_URL", "TOUCHGRASS_CONVEX_URL"]);
  const current = readFileSync(localEnvironmentPath, "utf8");
  const next = current
    .split(/\r?\n/)
    .filter((line) => {
      const name = /^\s*(?:export\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*=/.exec(line)?.[1];
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
  const result = await runCaptured(["dev", "--once", "--tail-logs", "disable"], {
    environment: anonymousLocalEnvironment(),
  });
  if (!existsSync(localEnvironmentPath)) {
    throw new Error(failureMessage("Local Convex setup failed.", result));
  }
  const environment = readLocalDevelopmentEnvironment();
  if (
    !environment.CONVEX_DEPLOYMENT?.trim() ||
    !environment.CONVEX_SITE_URL?.trim() ||
    !environment.CONVEX_URL?.trim() ||
    developmentTarget(environment) !== "local"
  ) {
    throw new Error(failureMessage("Local Convex setup failed.", result));
  }
}

async function ensureLocalBetterAuthSecret() {
  const listed = await runCaptured(["env", "list", "--names-only"]);
  if (listed.exitCode !== 0) {
    throw new Error(failureMessage("The local Convex environment could not be read.", listed));
  }
  if (!listed.stdout.split(/\r?\n/).includes("BETTER_AUTH_SECRET")) {
    const secret = randomBytes(48).toString("base64url");
    const result = await runCaptured(["env", "set", "BETTER_AUTH_SECRET"], {
      input: `${secret}\n`,
    });
    if (result.exitCode !== 0) {
      throw new Error(failureMessage("The local Better Auth secret could not be set.", result));
    }
  }
}

async function pushSelectedBackend() {
  const result = await runCaptured(["dev", "--once", "--tail-logs", "disable"]);
  if (result.exitCode !== 0) {
    throw new Error(failureMessage("The selected Convex backend could not be prepared.", result));
  }
}

if (process.argv.length > 2) {
  throw new Error(`Unknown argument(s): ${process.argv.slice(2).join(", ")}`);
}

await installDependencies();
writeFileSync(localEnvironmentPath, "", { mode: 0o600 });
chmodSync(localEnvironmentPath, 0o600);
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
