#!/usr/bin/env bun

import { execFileSync, spawnSync } from "node:child_process";
import { existsSync, mkdirSync } from "node:fs";
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
  "codex-usage-debug",
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
        "Use: bun debug:codex-usage [--fresh] [--passes <1-100>]",
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

function seedDebugDatabase(liveDatabase: string) {
  if (existsSync(debugDatabase) || !existsSync(liveDatabase)) return;
  mkdirSync(debugDirectory, { recursive: true });
  const backup = spawnSync(
    "sqlite3",
    [liveDatabase, `.backup '${debugDatabase}'`],
    {
      cwd: workspaceRoot,
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
    },
  );
  if (backup.status !== 0) {
    throw new Error("The private SQLite debug snapshot could not be created.");
  }
}

function sqliteLiteral(value: string) {
  return `'${value.replaceAll("'", "''")}'`;
}

const currentAccountCacheSchema = [
  "DROP TABLE IF EXISTS codex_account_usage_days;",
  "DROP TABLE IF EXISTS codex_account_usage_meta;",
  "CREATE TABLE codex_account_usage_meta (singleton INTEGER PRIMARY KEY NOT NULL CHECK(singleton = 1), refreshed_at TEXT NOT NULL);",
  "CREATE TABLE codex_account_usage_days (day TEXT PRIMARY KEY NOT NULL, tokens INTEGER NOT NULL, observed_at TEXT NOT NULL);",
];

function accountCacheShape(database: string) {
  const result = spawnSync(
    "sqlite3",
    [
      database,
      [
        "SELECT group_concat(name, ',') FROM pragma_table_info('codex_account_usage_days');",
        "SELECT group_concat(name, ',') FROM pragma_table_info('codex_account_usage_meta');",
      ].join(" "),
    ],
    { cwd: workspaceRoot, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] },
  );
  if (result.status !== 0) return null;
  const [dayColumns, metaColumns] = result.stdout.trim().split("\n");
  if (
    dayColumns === "day,tokens,observed_at" &&
    metaColumns === "singleton,refreshed_at"
  ) {
    return "current";
  }
  if (dayColumns === "day,tokens" && metaColumns === "singleton,observed_at") {
    return "legacy";
  }
  return null;
}

function refreshAccountCache(liveDatabase: string) {
  if (!existsSync(liveDatabase)) {
    console.error(
      "[TouchGrassBar][codex-usage] debug_account_cache=unavailable",
    );
    return;
  }
  mkdirSync(debugDirectory, { recursive: true });
  const sourceShape = accountCacheShape(liveDatabase);
  if (sourceShape === null) {
    const clear = spawnSync(
      "sqlite3",
      [
        debugDatabase,
        ["BEGIN IMMEDIATE;", ...currentAccountCacheSchema, "COMMIT;"].join(" "),
      ],
      {
        cwd: workspaceRoot,
        encoding: "utf8",
        stdio: ["ignore", "pipe", "pipe"],
      },
    );
    if (clear.status !== 0) {
      throw new Error("The private account usage cache could not be cleared.");
    }
    console.error(
      "[TouchGrassBar][codex-usage] debug_account_cache=unavailable",
    );
    return;
  }
  const copyAccountDays =
    sourceShape === "current"
      ? "INSERT INTO codex_account_usage_days(day, tokens, observed_at) SELECT day, tokens, observed_at FROM live.codex_account_usage_days;"
      : "INSERT INTO codex_account_usage_days(day, tokens, observed_at) SELECT day, tokens, (SELECT observed_at FROM live.codex_account_usage_meta WHERE singleton = 1) FROM live.codex_account_usage_days;";
  const copyAccountMeta =
    sourceShape === "current"
      ? "INSERT INTO codex_account_usage_meta(singleton, refreshed_at) SELECT singleton, refreshed_at FROM live.codex_account_usage_meta;"
      : "INSERT INTO codex_account_usage_meta(singleton, refreshed_at) SELECT singleton, observed_at FROM live.codex_account_usage_meta;";
  const refresh = spawnSync(
    "sqlite3",
    [
      debugDatabase,
      [
        `ATTACH DATABASE ${sqliteLiteral(liveDatabase)} AS live;`,
        "BEGIN IMMEDIATE;",
        ...currentAccountCacheSchema,
        copyAccountDays,
        copyAccountMeta,
        "COMMIT;",
        "DETACH DATABASE live;",
      ].join(" "),
    ],
    {
      cwd: workspaceRoot,
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
    },
  );
  if (refresh.status !== 0) {
    throw new Error("The private account usage cache could not be refreshed.");
  }
  console.error("[TouchGrassBar][codex-usage] debug_account_cache=refreshed");
}

function clearLocalIndex() {
  if (!existsSync(debugDatabase)) return;
  const reset = spawnSync(
    "sqlite3",
    [
      debugDatabase,
      [
        "PRAGMA foreign_keys = ON;",
        "DELETE FROM codex_usage_files;",
        "DELETE FROM codex_usage_file_days;",
        "DELETE FROM codex_usage_file_model_days;",
        "DELETE FROM codex_usage_index_meta;",
      ].join(" "),
    ],
    {
      cwd: workspaceRoot,
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
    },
  );
  if (reset.status !== 0) {
    throw new Error("The private SQLite debug index could not be reset.");
  }
  console.error("[TouchGrassBar][codex-usage] debug_index=cleared");
}

function main() {
  const userHome = homedir();
  if (!userHome || userHome === "/") {
    throw new Error("A safe user home directory could not be resolved.");
  }
  const liveDatabase = join(
    userHome,
    "Library",
    "Application Support",
    developmentNamespace(),
    "touchgrassbar.sqlite3",
  );
  const options = requestedOptions();
  if (options.fresh) {
    console.error("[TouchGrassBar][codex-usage] debug_index=reset_requested");
  }
  seedDebugDatabase(liveDatabase);
  refreshAccountCache(liveDatabase);
  if (options.fresh) clearLocalIndex();
  mkdirSync(debugDirectory, { recursive: true });

  const result = spawnSync(
    "cargo",
    ["run", "--manifest-path", manifestPath, "--bin", "debug_codex_usage"],
    {
      cwd: workspaceRoot,
      env: {
        ...Bun.env,
        CODEX_HOME: Bun.env.CODEX_HOME || join(userHome, ".codex"),
        TOUCHGRASS_USAGE_DEBUG_DATABASE: debugDatabase,
        TOUCHGRASS_USAGE_DEBUG_PASSES: String(options.passes),
      },
      stdio: "inherit",
    },
  );
  if (result.status !== 0) process.exit(result.status ?? 1);
}

main();
