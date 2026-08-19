#!/usr/bin/env bun

import { spawnSync } from "node:child_process";
import { mkdirSync } from "node:fs";
import { join, resolve } from "node:path";

const workspaceRoot = resolve(import.meta.dir, "..");
const manifestPath = join(workspaceRoot, "apps", "desktop", "src-tauri", "Cargo.toml");
const probeDirectory = join(
  workspaceRoot,
  "apps",
  "desktop",
  "src-tauri",
  "target",
  "claude-quota-debug",
);

function main() {
  if (process.argv.length > 2) {
    throw new Error("Use: bun debug:claude-quota");
  }
  mkdirSync(probeDirectory, { recursive: true });
  console.error("[TouchGrassBar][claude-quota-report] debug_source=claude-cli snapshot=direct");
  const result = spawnSync(
    "cargo",
    ["run", "--quiet", "--manifest-path", manifestPath, "--bin", "debug_claude_quota"],
    {
      cwd: workspaceRoot,
      env: {
        ...Bun.env,
        TOUCHGRASS_CLAUDE_DEBUG_DIRECTORY: probeDirectory,
      },
      stdio: "inherit",
    },
  );
  if (result.status !== 0) process.exit(result.status ?? 1);
}

main();
