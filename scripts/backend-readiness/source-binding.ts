import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { BACKEND_POLICY_VERSION } from "../../packages/backend/convex/model/policy";
import { BOARD_KEY_VERSION } from "../../packages/backend/convex/model/values";
import type { BackendBinding } from "./evidence";

function command(workspaceRoot: string, args: string[]) {
  return execFileSync("git", args, {
    cwd: workspaceRoot,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "ignore"],
  }).trim();
}

function sha256(path: string) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

export function sourceBinding(
  workspaceRoot: string,
  options: { allowedDirtyPaths?: string[] } = {},
): BackendBinding {
  const dirtyPaths = command(workspaceRoot, ["status", "--porcelain", "--untracked-files=all"])
    .split("\n")
    .filter(Boolean)
    .map((line) => line.slice(3));
  const allowedDirtyPaths = new Set(options.allowedDirtyPaths ?? []);
  if (dirtyPaths.some((path) => !allowedDirtyPaths.has(path))) {
    throw new Error("Backend readiness requires a clean source checkout");
  }
  const commit = command(workspaceRoot, ["rev-parse", "HEAD"]);
  if (!/^[0-9a-f]{40}$/u.test(commit)) {
    throw new Error("Backend readiness could not bind the source commit");
  }
  return {
    boardKeyVersion: BOARD_KEY_VERSION,
    commit,
    lockHash: sha256(resolve(workspaceRoot, "bun.lock")),
    policyVersion: BACKEND_POLICY_VERSION,
    schemaHash: sha256(resolve(workspaceRoot, "packages/backend/convex/schema.ts")),
  };
}
