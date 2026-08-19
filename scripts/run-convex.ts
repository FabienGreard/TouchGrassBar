#!/usr/bin/env bun

import { resolve } from "node:path";

import { convexCommandEnvironment } from "./convex-command-environment";
import { coordinatedProcessExitCode, type CoordinatedSignal } from "./coordinated-process-exit";
import { readLocalDevelopmentEnvironment } from "./development-environment";

const workspaceRoot = resolve(import.meta.dir, "..");
const convexCli = resolve(workspaceRoot, "packages/backend/node_modules/convex/bin/main.js");
const argumentsList = process.argv.slice(2);
const environment = convexCommandEnvironment(
  argumentsList,
  process.env,
  readLocalDevelopmentEnvironment(),
);

const child = Bun.spawn([process.execPath, convexCli, ...argumentsList], {
  cwd: workspaceRoot,
  env: environment,
  stdin: "inherit",
  stderr: "inherit",
  stdout: "inherit",
});

let stoppingSignal: CoordinatedSignal | null = null;
for (const signal of ["SIGINT", "SIGTERM"] as const) {
  process.on(signal, () => {
    if (stoppingSignal !== null) return;
    stoppingSignal = signal;
    if (child.exitCode === null) child.kill(signal);
  });
}

process.exitCode = coordinatedProcessExitCode(await child.exited, stoppingSignal);
