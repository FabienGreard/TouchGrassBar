import { randomBytes } from "node:crypto";
import { chmodSync, existsSync, readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";

const workspaceRoot = resolve(import.meta.dir, "..");
const convexCli = resolve(
  workspaceRoot,
  "packages/backend/node_modules/convex/bin/main.js",
);
const localConfigPath = resolve(
  workspaceRoot,
  ".convex/local/default/config.json",
);
const localEnvPath = resolve(workspaceRoot, ".env.local");
const waitTimeoutMs = 60_000;

type LocalConfig = {
  deploymentName?: unknown;
  ports?: {
    cloud?: unknown;
    site?: unknown;
  };
};

type CommandResult = {
  exitCode: number;
  stdout: string;
};

function printHelp() {
  console.log(`Manage the worktree-local Convex backend.

Usage:
  bun scripts/convex-local.ts setup
  bun scripts/convex-local.ts dev

Commands:
  setup  Create or select an anonymous local deployment, configure its
         private Better Auth secret, and push the backend once.
  dev    Start the selected local deployment and watch for changes.

This script never selects a cloud dev or production deployment.`);
}

function convexEnvironment() {
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
  return environment;
}

function commandArguments(arguments_: string[]) {
  return [process.execPath, convexCli, ...arguments_];
}

async function runCaptured(
  arguments_: string[],
  options: { input?: string } = {},
): Promise<CommandResult> {
  const child = Bun.spawn(commandArguments(arguments_), {
    cwd: workspaceRoot,
    env: convexEnvironment(),
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

function localPorts() {
  if (!existsSync(localConfigPath)) {
    throw new Error("The worktree-local Convex configuration is missing.");
  }
  const config = JSON.parse(readFileSync(localConfigPath, "utf8")) as LocalConfig;
  const cloud = config.ports?.cloud;
  const site = config.ports?.site;
  if (
    typeof cloud !== "number" ||
    !Number.isInteger(cloud) ||
    cloud <= 0 ||
    cloud > 65_535 ||
    typeof site !== "number" ||
    !Number.isInteger(site) ||
    site <= 0 ||
    site > 65_535
  ) {
    throw new Error("The worktree-local Convex ports are invalid.");
  }
  return { cloud, site };
}

function localDeployment() {
  const config = JSON.parse(readFileSync(localConfigPath, "utf8")) as LocalConfig;
  if (
    typeof config.deploymentName !== "string" ||
    !/^anonymous-[A-Za-z0-9_-]+$/.test(config.deploymentName)
  ) {
    throw new Error("The Convex deployment is not worktree-local anonymous.");
  }
  return config.deploymentName;
}

function setEnvironmentValue(source: string, name: string, value: string) {
  const line = `${name}=${value}`;
  const pattern = new RegExp(`^${name}=.*$`, "m");
  if (pattern.test(source)) return source.replace(pattern, line);
  const separator = source.length === 0 || source.endsWith("\n") ? "" : "\n";
  return `${source}${separator}${line}\n`;
}

function configureDesktopUrls() {
  const { cloud, site } = localPorts();
  const deployment = localDeployment();
  const current = existsSync(localEnvPath)
    ? readFileSync(localEnvPath, "utf8")
    : "";
  const selected = current.match(/^CONVEX_DEPLOYMENT=(.+)$/m)?.[1];
  if (selected && !selected.startsWith("anonymous:")) {
    throw new Error(
      "Refusing to replace a non-local Convex deployment selection.",
    );
  }
  const withDeployment = setEnvironmentValue(
    current,
    "CONVEX_DEPLOYMENT",
    `anonymous:${deployment}`,
  );
  const withConvex = setEnvironmentValue(
    withDeployment,
    "CONVEX_URL",
    `http://127.0.0.1:${cloud}`,
  );
  const withSite = setEnvironmentValue(
    withConvex,
    "CONVEX_SITE_URL",
    `http://127.0.0.1:${site}`,
  );
  const withDesktopConvex = setEnvironmentValue(
    withSite,
    "TOUCHGRASS_CONVEX_URL",
    `http://127.0.0.1:${cloud}`,
  );
  const withAuth = setEnvironmentValue(
    withDesktopConvex,
    "TOUCHGRASS_AUTH_SITE_URL",
    `http://127.0.0.1:${site}`,
  );
  writeFileSync(localEnvPath, withAuth, { mode: 0o600 });
  chmodSync(localEnvPath, 0o600);
}

function requireLocalDeployment() {
  if (!existsSync(localConfigPath)) {
    throw new Error("The worktree-local Convex configuration is missing.");
  }
  configureDesktopUrls();
}

async function waitForLocalBackend(backend: ReturnType<typeof Bun.spawn>) {
  const startedAt = Date.now();
  while (Date.now() - startedAt < waitTimeoutMs) {
    if (backend.exitCode !== null) {
      throw new Error("The worktree-local Convex backend stopped early.");
    }
    if (existsSync(localConfigPath)) configureDesktopUrls();
    const result = await runCaptured(["env", "list", "--names-only"]);
    if (result.exitCode === 0) return result.stdout;
    await Bun.sleep(500);
  }
  throw new Error("The worktree-local Convex backend did not become ready.");
}

async function configureLocalSecret() {
  const backend = Bun.spawn(
    commandArguments([
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
      env: convexEnvironment(),
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
        throw new Error("The local Better Auth secret could not be configured.");
      }
    }
  } finally {
    backend.kill();
    await backend.exited;
    await stdout;
    await stderr;
  }
}

async function setup() {
  if (existsSync(localConfigPath)) requireLocalDeployment();
  await configureLocalSecret();
  requireLocalDeployment();
  const pushArguments = ["dev", "--once", "--tail-logs", "disable"];
  let pushResult = await runCaptured(pushArguments);
  if (pushResult.exitCode !== 0) {
    await Bun.sleep(500);
    pushResult = await runCaptured(pushArguments);
  }
  if (pushResult.exitCode !== 0) {
    throw new Error(
      "Local Convex backend push failed. No cloud deployment was selected.",
    );
  }
  console.log("Worktree-local Convex setup is ready.");
  console.log("Cloud dev and production were not selected.");
  console.log("Run `bun run convex:dev` while testing the native app.");
}

async function dev() {
  if (!existsSync(localConfigPath)) {
    throw new Error("Run `bun run convex:setup:local` first.");
  }
  requireLocalDeployment();
  const child = Bun.spawn(commandArguments(["dev"]), {
    cwd: workspaceRoot,
    env: convexEnvironment(),
    stdin: "inherit",
    stderr: "inherit",
    stdout: "inherit",
  });
  for (const signal of ["SIGINT", "SIGTERM"] as const) {
    process.on(signal, () => child.kill(signal));
  }
  const exitCode = await child.exited;
  if (exitCode !== 0) process.exit(exitCode);
}

const command = process.argv[2];
if (command === "--help" || command === "-h") {
  printHelp();
} else if (command === "setup") {
  await setup();
} else if (command === "dev") {
  await dev();
} else {
  printHelp();
  process.exit(1);
}
