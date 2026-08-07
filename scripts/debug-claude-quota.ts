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

function sqliteLiteral(value: string) {
  return `'${value.replaceAll("'", "''")}'`;
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

function snapshotSourceDatabase(sourceDatabase: string) {
  if (!existsSync(sourceDatabase)) return false;
  const sourceTables = spawnSync(
    "sqlite3",
    [
      sourceDatabase,
      "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name IN ('touchgrassbar_schema_versions', 'claude_quota_observation', 'claude_response_cursors');",
    ],
    {
      cwd: workspaceRoot,
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
    },
  );
  if (sourceTables.status !== 0 || sourceTables.stdout.trim() !== "3") {
    return false;
  }
  const snapshot = spawnSync(
    "sqlite3",
    [
      debugDatabase,
      [
        "CREATE TABLE touchgrassbar_schema_versions (module TEXT PRIMARY KEY, version INTEGER NOT NULL CHECK (version >= 1));",
        "CREATE TABLE claude_quota_observation (singleton INTEGER PRIMARY KEY NOT NULL CHECK(singleton = 1), observed_at TEXT NOT NULL, five_hour_used_percentage REAL NOT NULL CHECK(five_hour_used_percentage >= 0 AND five_hour_used_percentage <= 100), five_hour_resets_at INTEGER NOT NULL CHECK(five_hour_resets_at > 0), seven_day_used_percentage REAL NOT NULL CHECK(seven_day_used_percentage >= 0 AND seven_day_used_percentage <= 100), seven_day_resets_at INTEGER NOT NULL CHECK(seven_day_resets_at > 0));",
        "CREATE TABLE claude_response_cursors (session_id TEXT PRIMARY KEY NOT NULL, total_api_duration_ms INTEGER NOT NULL CHECK(total_api_duration_ms > 0), observed_at TEXT NOT NULL);",
        `ATTACH DATABASE ${sqliteLiteral(sourceDatabase)} AS source;`,
        "INSERT INTO touchgrassbar_schema_versions(module, version) SELECT module, version FROM source.touchgrassbar_schema_versions WHERE module = 'claude-quota-capture';",
        "INSERT INTO claude_quota_observation SELECT * FROM source.claude_quota_observation;",
        "DETACH DATABASE source;",
      ].join(" "),
    ],
    {
      cwd: workspaceRoot,
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
    },
  );
  if (snapshot.status !== 0) {
    throw new Error("The reduced Claude quota snapshot could not be created.");
  }
  return true;
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
  let snapshotAvailable = false;
  if (!options.fixture) {
    source = options.release ? "release" : "development";
    const namespace = options.release
      ? "app.touchgrass.bar"
      : developmentNamespace();
    snapshotAvailable = snapshotSourceDatabase(
      join(
        userHome,
        "Library",
        "Application Support",
        namespace,
        "touchgrassbar.sqlite3",
      ),
    );
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
        TOUCHGRASS_CLAUDE_DEBUG_FIXTURE: options.fixture ? "1" : "0",
      },
      stdio: "inherit",
    },
  );
  if (result.status !== 0) process.exit(result.status ?? 1);
}

main();
