import { execFileSync } from "node:child_process";
import { mkdirSync } from "node:fs";
import { createServer } from "node:net";
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

type RunnerOptions = {
  accent?: string | undefined;
  browserOnly: boolean;
  label?: string | undefined;
  port?: number | undefined;
};

function printHelp() {
  console.log(`Run an isolated TouchGrassBar development instance.

Usage:
  bun run desktop [--label <name>] [--accent <color>] [--port <port>]
  bun run desktop:preview [--label <name>] [--accent <color>] [--port <port>]

Options:
  --label    Override the branch-derived visible name.
  --accent   Use one of: ${devAccents.join(", ")}.
  --port     Use one specific localhost port.
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
    if (argument === "--port") {
      const value = Number(argumentValue(argumentsList, index, argument));
      if (!Number.isInteger(value) || value < 1_024 || value > 65_535) {
        throw new Error("--port must be an integer from 1024 through 65535.");
      }
      options.port = value;
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

async function availablePort(preferred: number, explicit: boolean) {
  if (explicit) {
    if (await portIsAvailable(preferred)) return preferred;
    throw new Error(`The requested development port ${preferred} is in use.`);
  }

  for (let offset = 0; offset < 1_000; offset += 1) {
    const candidate = 15_000 + ((preferred - 15_000 + offset) % 1_000);
    if (await portIsAvailable(candidate)) return candidate;
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
    port: options.port,
    worktreeSeed,
  });
  const port = await availablePort(requested.port, options.port !== undefined);
  const instance = { ...requested, port };
  const serializedInstance = JSON.stringify(instance);
  const environment = {
    ...Bun.env,
    TOUCHGRASS_DEV_INSTANCE_LABEL: instance.label,
    TOUCHGRASS_DEV_INSTANCE_TAG: instance.tag,
    VITE_TOUCHGRASS_DEV_INSTANCE: serializedInstance,
  };

  printInstance(instance, options.browserOnly);
  if (options.browserOnly) {
    const child = Bun.spawn(
      ["bun", "run", "dev", "--", "--port", String(instance.port)],
      { cwd: desktopRoot, env: environment, stderr: "inherit", stdout: "inherit" },
    );
    process.exitCode = await child.exited;
    return;
  }

  await writeTauriConfig(instance);
  const child = Bun.spawn(
    ["bun", "run", "tauri", "dev", "--config", generatedConfigPath],
    { cwd: desktopRoot, env: environment, stderr: "inherit", stdout: "inherit" },
  );
  process.exitCode = await child.exited;
}

await main();
