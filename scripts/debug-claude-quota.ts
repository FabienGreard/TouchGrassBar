#!/usr/bin/env bun

import { execFileSync, spawnSync } from "node:child_process";
import { existsSync, mkdirSync, rmSync } from "node:fs";
import { homedir } from "node:os";
import { join, resolve } from "node:path";

import { resolveDevInstance } from "../apps/desktop/src/dev/dev-instance";

const workspaceRoot = resolve(import.meta.dir, "..");
const manifestPath = join(
  workspaceRoot,
  "apps",
  "desktop",
  "src-tauri",
  "Cargo.toml",
);
const debugDirectory = join(
  workspaceRoot,
  "apps",
  "desktop",
  "src-tauri",
  "target",
  "claude-quota-debug",
);
const debugDatabase = join(debugDirectory, "touchgrassbar.sqlite3");

function gitText(argumentsList: string[]) {
  return execFileSync("git", argumentsList, {
    cwd: workspaceRoot,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "ignore"],
  }).trim();
}

function developmentNamespace() {
  const branch = gitText(["branch", "--show-current"]);
  return resolveDevInstance({
    branch: branch || `detached-${gitText(["rev-parse", "--short", "HEAD"])}`,
    worktreeSeed: gitText(["rev-parse", "--show-toplevel"]),
  }).namespace;
}

function requestedOptions() {
  let fixture = false;
  let release = false;
  for (const argument of process.argv.slice(2)) {
    if (argument === "--fixture") fixture = true;
    else if (argument === "--release") release = true;
    else {
      throw new Error(
        "Use: bun debug:claude-quota [--fixture | --release]",
      );
    }
  }
  if (fixture && release) {
    throw new Error("The fixture and release sources cannot be combined.");
  }
  return { fixture, release };
}

function clearDebugDatabase() {
  for (const path of [
    debugDatabase,
    `${debugDatabase}-shm`,
    `${debugDatabase}-wal`,
  ]) {
    rmSync(path, { force: true });
  }
}

function main() {
  const userHome = homedir();
  if (!userHome || userHome === "/") {
    throw new Error("A safe user home directory could not be resolved.");
  }
  const options = requestedOptions();
  mkdirSync(debugDirectory, { recursive: true });
  clearDebugDatabase();

  let source = "fixture";
  let sourceDatabase: string | undefined;
  let snapshotAvailable = false;
  if (!options.fixture) {
    source = options.release ? "release" : "development";
    const namespace = options.release
      ? "app.touchgrass.bar"
      : developmentNamespace();
    sourceDatabase = join(
      userHome,
      "Library",
      "Application Support",
      namespace,
      "touchgrassbar.sqlite3",
    );
    snapshotAvailable = existsSync(sourceDatabase);
  }
  console.error(
    `[TouchGrassBar][claude-quota-report] debug_source=${source} snapshot=${options.fixture ? "synthetic" : snapshotAvailable ? "loaded" : "unavailable"}`,
  );

  const result = spawnSync(
    "cargo",
    [
      "run",
      "--quiet",
      "--manifest-path",
      manifestPath,
      "--bin",
      "debug_claude_quota",
    ],
    {
      cwd: workspaceRoot,
      env: {
        ...Bun.env,
        TOUCHGRASS_CLAUDE_DEBUG_DATABASE: debugDatabase,
        TOUCHGRASS_CLAUDE_DEBUG_SOURCE_DATABASE: snapshotAvailable
          ? sourceDatabase
          : undefined,
        TOUCHGRASS_CLAUDE_DEBUG_FIXTURE: options.fixture ? "1" : "0",
      },
      stdio: "inherit",
    },
  );
  if (result.status !== 0) process.exit(result.status ?? 1);
}

main();
