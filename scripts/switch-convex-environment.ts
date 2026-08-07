#!/usr/bin/env bun

import { chmodSync, existsSync, readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";

const workspaceRoot = resolve(import.meta.dir, "..");
const target = process.argv[2];

if (target !== "dev" && target !== "prod") {
  throw new Error("Select the dev or prod Convex environment.");
}

if (process.argv.length > 3) {
  throw new Error(`Unknown argument(s): ${process.argv.slice(3).join(", ")}`);
}

const source = resolve(workspaceRoot, `.env.${target}`);
const destination = resolve(workspaceRoot, ".env.local");

if (!existsSync(source)) {
  throw new Error(`The .env.${target} Convex environment does not exist.`);
}

const profile = readFileSync(source, "utf8").replace(/^\s+|\s+$/g, "");
const environment = `TOUCHGRASS_CONVEX_TARGET=${target}\n${profile}\n`;
writeFileSync(destination, environment, { mode: 0o600 });
chmodSync(destination, 0o600);

console.log(`Selected the ${target} Convex environment.`);
