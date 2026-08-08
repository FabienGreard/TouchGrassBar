#!/usr/bin/env bun

import { spawnSync } from "node:child_process";
import {
  chmodSync,
  lstatSync,
  mkdirSync,
  realpathSync,
  unlinkSync,
} from "node:fs";
import { homedir } from "node:os";
import { join, parse, resolve } from "node:path";

import { resolveDevInstance } from "../apps/desktop/src/dev/dev-instance";

const workspaceRoot = resolve(import.meta.dir, "..");
const manifestPath = join(
  workspaceRoot,
  "apps",
  "desktop",
  "src-tauri",
  "Cargo.toml",
);
const targetDirectory = join(
  workspaceRoot,
  "apps",
  "desktop",
  "src-tauri",
  "target",
);
const debugDirectory = join(
  targetDirectory,
  "claude-usage-debug",
);
const debugDatabase = join(debugDirectory, "touchgrassbar.sqlite3");
const privateIndexFiles = [
  debugDatabase,
  `${debugDatabase}-journal`,
  `${debugDatabase}-shm`,
  `${debugDatabase}-wal`,
  `${debugDatabase}.claude-usage-v0.backup`,
  `${debugDatabase}.claude-usage-v0.backup.partial`,
  `${debugDatabase}.claude-usage-v1.backup`,
  `${debugDatabase}.claude-usage-v1.backup.partial`,
  `${debugDatabase}.claude-usage-v2.backup`,
  `${debugDatabase}.claude-usage-v2.backup.partial`,
];

function gitText(argumentsList: string[]) {
  const result = spawnSync("git", argumentsList, {
    cwd: workspaceRoot,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "ignore"],
  });
  if (result.status !== 0) {
    throw new Error("The development namespace could not be resolved.");
  }
  return result.stdout.trim();
}

function developmentNamespace() {
  const branch = gitText(["branch", "--show-current"]);
  return resolveDevInstance({
    branch: branch || `detached-${gitText(["rev-parse", "--short", "HEAD"])}`,
    worktreeSeed: gitText(["rev-parse", "--show-toplevel"]),
  }).namespace;
}

function requestedOptions() {
  const argumentsList = process.argv.slice(2);
  let fresh = false;
  let passes = 1;
  for (let index = 0; index < argumentsList.length; index += 1) {
    const argument = argumentsList[index];
    if (argument === "--fresh") {
      fresh = true;
      continue;
    }
    if (argument !== "--passes" || index + 1 >= argumentsList.length) {
      throw new Error(
        "Use: bun debug:claude-usage [--fresh] [--passes <1-100>]",
      );
    }
    passes = Number(argumentsList[index + 1]);
    index += 1;
  }
  if (!Number.isInteger(passes) || passes < 1 || passes > 100) {
    throw new Error("The pass count must be from 1 through 100.");
  }
  return { fresh, passes };
}

function claudeConfigRoot(userHome: string) {
  const configured = Bun.env.CLAUDE_CONFIG_DIR;
  const root = configured
    ? resolve(userHome, configured)
    : join(userHome, ".claude");
  if (root === parse(root).root) {
    throw new Error("A safe Claude configuration directory could not be resolved.");
  }
  return root;
}

function optionalMetadata(path: string) {
  try {
    return lstatSync(path);
  } catch (error) {
    const code =
      error && typeof error === "object" && "code" in error
        ? error.code
        : undefined;
    if (code === "ENOENT") return undefined;
    throw new Error("The private SQLite debug path could not be inspected.");
  }
}

function requireRealDirectory(path: string) {
  const metadata = lstatSync(path);
  if (
    !metadata.isDirectory() ||
    metadata.isSymbolicLink() ||
    realpathSync(path) !== resolve(path)
  ) {
    throw new Error("The private SQLite debug directory is unsafe.");
  }
}

function ensurePrivateDebugDirectory() {
  if (!optionalMetadata(targetDirectory)) {
    mkdirSync(targetDirectory, { recursive: true, mode: 0o700 });
  }
  requireRealDirectory(targetDirectory);
  if (!optionalMetadata(debugDirectory)) {
    mkdirSync(debugDirectory, { mode: 0o700 });
  }
  requireRealDirectory(debugDirectory);
  chmodSync(debugDirectory, 0o700);
  if ((lstatSync(debugDirectory).mode & 0o777) !== 0o700) {
    throw new Error("The private SQLite debug directory is not private.");
  }
}

function requirePrivateIndexFile(path: string, metadata = lstatSync(path)) {
  if (
    !metadata.isFile() ||
    metadata.isSymbolicLink() ||
    metadata.nlink !== 1
  ) {
    throw new Error("The private SQLite debug index is unsafe.");
  }
}

function protectPrivateIndexFiles() {
  for (const path of privateIndexFiles) {
    const metadata = optionalMetadata(path);
    if (!metadata) continue;
    requirePrivateIndexFile(path, metadata);
    chmodSync(path, 0o600);
    if ((lstatSync(path).mode & 0o777) !== 0o600) {
      throw new Error("The private SQLite debug index is not private.");
    }
  }
}

function clearDebugDatabase() {
  let removed = false;
  for (const path of privateIndexFiles) {
    const metadata = optionalMetadata(path);
    if (!metadata) continue;
    requirePrivateIndexFile(path, metadata);
    unlinkSync(path);
    removed = true;
  }
  if (removed) {
    console.error("[TouchGrassBar][claude-usage] debug_index=cleared");
  }
}

function main() {
  process.umask(0o077);
  const rawUserHome = homedir();
  if (!rawUserHome) {
    throw new Error("A safe user home directory could not be resolved.");
  }
  const userHome = resolve(rawUserHome);
  if (userHome === parse(userHome).root) {
    throw new Error("A safe user home directory could not be resolved.");
  }
  const options = requestedOptions();
  ensurePrivateDebugDirectory();
  protectPrivateIndexFiles();
  if (options.fresh) {
    console.error("[TouchGrassBar][claude-usage] debug_index=reset_requested");
    clearDebugDatabase();
  }

  const applicationSupport = join(
    userHome,
    "Library",
    "Application Support",
    developmentNamespace(),
  );
  const result = spawnSync(
    "cargo",
    [
      "run",
      "--quiet",
      "--manifest-path",
      manifestPath,
      "--bin",
      "debug_claude_usage",
    ],
    {
      cwd: workspaceRoot,
      env: {
        ...Bun.env,
        TOUCHGRASS_CLAUDE_USAGE_CONFIG_ROOT: claudeConfigRoot(userHome),
        TOUCHGRASS_CLAUDE_USAGE_DEBUG_DATABASE: debugDatabase,
        TOUCHGRASS_CLAUDE_USAGE_DEBUG_PASSES: String(options.passes),
        TOUCHGRASS_CLAUDE_USAGE_PROBE_DIRECTORY: join(
          applicationSupport,
          "claude-quota-probe",
        ),
      },
      stdio: "inherit",
    },
  );
  protectPrivateIndexFiles();
  if (result.status !== 0) process.exit(result.status ?? 1);
}

main();
