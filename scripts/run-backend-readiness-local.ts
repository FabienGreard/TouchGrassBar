#!/usr/bin/env bun

import { writeFileSync } from "node:fs";
import { resolve } from "node:path";

import type { CheckReceipt } from "./backend-readiness/evidence";
import type { BackendReadinessPreflight } from "./backend-readiness/preflight";
import { sourceBinding } from "./backend-readiness/source-binding";

function outputPath() {
  const outputIndex = process.argv.indexOf("--output");
  const output = outputIndex === -1 ? undefined : process.argv[outputIndex + 1];
  if (!output || process.argv.length !== 4) {
    throw new Error("Usage: bun backend:readiness:local --output <path>");
  }
  return resolve(output);
}

async function runCheck(command: string[]): Promise<CheckReceipt> {
  const result = await Bun.spawn(command, {
    cwd: process.cwd(),
    stderr: "inherit",
    stdout: "inherit",
  }).exited;
  return {
    completedAt: new Date().toISOString(),
    counts: { commands: 1 },
    status: result === 0 ? "passed" : "failed",
  };
}

async function main() {
  const output = outputPath();
  const binding = sourceBinding(process.cwd());
  const automatedSuite = await runCheck(["bun", "run", "backend:readiness:automated"]);
  const migrationRehearsal =
    automatedSuite.status === "passed"
      ? await runCheck([
          "bun",
          "run",
          "--cwd",
          "packages/backend",
          "test",
          "--",
          "--run",
          "convex/sync.test.ts",
          "-t",
          "production-shaped migrations resume",
        ])
      : ({ completedAt: new Date().toISOString(), status: "skipped" } satisfies CheckReceipt);
  const completedBinding = sourceBinding(process.cwd());
  if (JSON.stringify(completedBinding) !== JSON.stringify(binding)) {
    throw new Error("Backend readiness source changed during the local preflight");
  }
  const preflight = {
    checks: { automatedSuite, migrationRehearsal },
    contractVersion: 1,
    generatedAt: new Date().toISOString(),
    sourceBinding: completedBinding,
  } satisfies BackendReadinessPreflight;
  writeFileSync(output, `${JSON.stringify(preflight, null, 2)}\n`, { mode: 0o600 });
  if (automatedSuite.status !== "passed" || migrationRehearsal.status !== "passed") {
    throw new Error("Backend readiness local preflight failed");
  }
  console.log("Backend readiness local preflight passed");
}

await main();
