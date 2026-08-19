#!/usr/bin/env bun

import { existsSync, rmSync } from "node:fs";
import { join, resolve } from "node:path";

import { workspaceRoot } from "./development-environment";

const argumentsSet = new Set(process.argv.slice(2));
const supportedArguments = new Set(["--dry-run"]);
const unknownArguments = [...argumentsSet].filter((argument) => !supportedArguments.has(argument));
if (unknownArguments.length > 0) {
  throw new Error(`Unknown argument(s): ${unknownArguments.join(", ")}`);
}

const dryRun = argumentsSet.has("--dry-run");
const targets = [
  { description: "Turbo cache", path: join(workspaceRoot, ".turbo") },
  {
    description: "desktop web build",
    path: join(workspaceRoot, "apps", "desktop", "dist"),
  },
  {
    description: "desktop native build",
    path: join(workspaceRoot, "apps", "desktop", "src-tauri", "target"),
  },
  {
    description: "desktop generated development configuration",
    path: join(workspaceRoot, "apps", "desktop", "src-tauri", ".dev-instance"),
  },
  {
    description: "landing build",
    path: join(workspaceRoot, "apps", "landing", "dist"),
  },
  {
    description: "landing cache",
    path: join(workspaceRoot, "apps", "landing", ".astro"),
  },
  {
    description: "Storybook build",
    path: join(workspaceRoot, "packages", "ui", "storybook-static"),
  },
];
const allowedTargets = new Set(targets.map((target) => resolve(target.path)));

for (const target of targets) {
  const path = resolve(target.path);
  if (!allowedTargets.has(path)) {
    throw new Error("A build cleanup target is outside the approved scope.");
  }
  if (!existsSync(path)) continue;
  if (dryRun) {
    console.log(`[dry-run] ${target.description}`);
  } else {
    rmSync(path, { force: true, recursive: true });
    console.log(`Removed ${target.description}.`);
  }
}

console.log(
  dryRun
    ? "Build cleanup dry run complete."
    : "Build output and caches are clean. Data and Keychain items were preserved.",
);
