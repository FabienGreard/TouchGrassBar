import { execFileSync } from "node:child_process";
import {
  closeSync,
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
  devAccents,
  resolveDevInstance,
  type DevAccent,
  type DevInstance,
} from "../apps/desktop/src/dev/dev-instance";

const workspaceRoot = resolve(import.meta.dir, "..");
const desktopRoot = join(workspaceRoot, "apps", "desktop");
const generatedConfigDirectory = join(
  desktopRoot,
  "src-tauri",
  ".dev-instance",
);
const generatedConfigPath = join(generatedConfigDirectory, "tauri.conf.json");
const portLockDirectory = join(tmpdir(), "touchgrassbar-dev-ports");

type RunnerOptions = {
  accent?: string | undefined;
  browserOnly: boolean;
  label?: string | undefined;
};

function printHelp() {
  console.log(`Run an isolated TouchGrassBar development instance.

Usage:
  bun run desktop [--label <name>] [--accent <color>]
  bun run desktop:preview [--label <name>] [--accent <color>]

Options:
  --label    Override the branch-derived visible name.
  --accent   Use one of: ${devAccents.join(", ")}.
  --browser  Start only the browser preview.
  --help     Show this help.`);
}

function argumentValue(argumentsList: string[], index: number, option: string) {
  const value = argumentsList[index + 1];
  if (!value || value.startsWith("--")) {
    throw new Error(`${option} requires a value.`);
  }
  return value;
}

function parseOptions(argumentsList: string[]): RunnerOptions | null {
  const options: RunnerOptions = { browserOnly: false };
  for (let index = 0; index < argumentsList.length; index += 1) {
    const argument = argumentsList[index];
    if (argument === "--help") return null;
    if (argument === "--browser") {
      options.browserOnly = true;
      continue;
    }
    if (argument === "--label") {
      options.label = argumentValue(argumentsList, index, argument);
      index += 1;
      continue;
    }
    if (argument === "--accent") {
      options.accent = argumentValue(argumentsList, index, argument);
      index += 1;
      continue;
    }
    throw new Error(`Unknown argument: ${argument ?? ""}`);
  }
  return options;
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

async function writeTauriConfig(instance: DevInstance) {
  mkdirSync(generatedConfigDirectory, { recursive: true });
  await Bun.write(
    generatedConfigPath,
    `${JSON.stringify(
      {
        app: { security: { devCsp: devCsp(instance.port) } },
        build: {
          beforeDevCommand: `bun run dev -- --port ${instance.port}`,
          devUrl: `http://127.0.0.1:${instance.port}`,
        },
        identifier: instance.identifier,
        productName: instance.productName,
      },
      null,
      2,
    )}\n`,
  );
}

function printInstance(instance: DevInstance, browserOnly: boolean) {
  console.log(`Development instance: ${instance.label}`);
  console.log(`Accent: ${instance.accent}`);
  console.log(`URL: http://127.0.0.1:${instance.port}`);
  console.log(browserOnly ? "Surface: browser preview" : "Surface: native app");
}

async function main() {
  const options = parseOptions(Bun.argv.slice(2));
  if (!options) {
    printHelp();
    return;
  }
  if (
    options.accent !== undefined &&
    !devAccents.includes(options.accent as DevAccent)
  ) {
    throw new Error(`--accent must be one of: ${devAccents.join(", ")}.`);
  }

  const branch = gitText(["branch", "--show-current"]);
  const worktreeSeed = gitText(["rev-parse", "--show-toplevel"]);
  const requested = resolveDevInstance({
    accent: options.accent ?? Bun.env.TOUCHGRASS_DEV_ACCENT,
    branch: branch || `detached-${gitText(["rev-parse", "--short", "HEAD"])}`,
    label: options.label ?? Bun.env.TOUCHGRASS_DEV_LABEL,
    worktreeSeed,
  });
  const portLease = await availablePort(requested.port);
  process.once("exit", portLease.release);
  const instance = { ...requested, port: portLease.port };
  const serializedInstance = JSON.stringify(instance);
  const environment = {
    ...Bun.env,
    TOUCHGRASS_DEV_INSTANCE_LABEL: instance.label,
    TOUCHGRASS_DEV_INSTANCE_TAG: instance.tag,
    VITE_TOUCHGRASS_DEV_INSTANCE: serializedInstance,
  };

  try {
    printInstance(instance, options.browserOnly);
    if (options.browserOnly) {
      const child = Bun.spawn(
        ["bun", "run", "dev", "--", "--port", String(instance.port)],
        {
          cwd: desktopRoot,
          env: environment,
          stderr: "inherit",
          stdout: "inherit",
        },
      );
      process.exitCode = await child.exited;
      return;
    }

    await writeTauriConfig(instance);
    const child = Bun.spawn(
      ["bun", "run", "tauri", "dev", "--config", generatedConfigPath],
      {
        cwd: desktopRoot,
        env: environment,
        stderr: "inherit",
        stdout: "inherit",
      },
    );
    process.exitCode = await child.exited;
  } finally {
    portLease.release();
  }
}

await main();
