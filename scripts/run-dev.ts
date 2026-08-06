#!/usr/bin/env bun

import { join } from "node:path";

import { developmentTarget, workspaceRoot } from "./development-environment";
import {
  claimDevelopmentRunnerLease,
  releaseDevelopmentRunnerLease,
} from "./development-runner-lease";

type Scope = "all" | "backend" | "desktop" | "landing" | "ui";
type ChildSpecification = {
  argumentsList: string[];
  cwd: string;
  environment?: Record<string, string>;
  label: string;
};

const scope = process.argv[2] as Scope | undefined;
const scopes = new Set<Scope>(["all", "backend", "desktop", "landing", "ui"]);
if (!scope || !scopes.has(scope) || process.argv.length > 3) {
  throw new Error("Development scope must be all, backend, desktop, landing, or ui.");
}

const target =
  scope === "all" || scope === "backend" || scope === "desktop"
    ? developmentTarget(Bun.env)
    : null;
claimDevelopmentRunnerLease();

const backend: ChildSpecification = {
  argumentsList: ["bun", "scripts/run-convex.ts", "dev"],
  cwd: workspaceRoot,
  label: "backend",
};
const desktop: ChildSpecification = {
  argumentsList: ["bun", "scripts/run-desktop-dev.ts"],
  cwd: workspaceRoot,
  label: "desktop",
};
const landing: ChildSpecification = {
  argumentsList: ["bunx", "--bun", "astro", "dev", "--ignore-lock"],
  cwd: join(workspaceRoot, "apps", "landing"),
  environment: { ASTRO_DEV_BACKGROUND: "0" },
  label: "landing",
};
const ui: ChildSpecification = {
  argumentsList: [
    "bunx",
    "--bun",
    "storybook",
    "dev",
    "-p",
    "6006",
    "--no-open",
  ],
  cwd: join(workspaceRoot, "packages", "ui"),
  label: "ui",
};

const specifications: Record<Scope, ChildSpecification[]> = {
  all: [backend, desktop, landing],
  backend: [backend],
  desktop: [backend, desktop],
  landing: [landing],
  ui: [ui],
};

console.log(`Development scope: ${scope}`);
if (target) console.log(`Convex target: ${target}`);

const children = specifications[scope].map((specification) => ({
  child: Bun.spawn(specification.argumentsList, {
    cwd: specification.cwd,
    env: { ...process.env, ...specification.environment },
    stdin: "inherit",
    stderr: "inherit",
    stdout: "inherit",
  }),
  label: specification.label,
}));

let stopping = false;
function releaseRunnerLease() {
  releaseDevelopmentRunnerLease(process.pid);
}

function stopChildren(signal: "SIGINT" | "SIGTERM" = "SIGTERM") {
  if (stopping) return;
  stopping = true;
  for (const { child } of children) {
    if (child.exitCode === null) child.kill(signal);
  }
}

for (const signal of ["SIGINT", "SIGTERM"] as const) {
  process.on(signal, () => {
    stopChildren(signal);
    releaseRunnerLease();
  });
}
process.once("exit", releaseRunnerLease);

try {
  const first = await Promise.race(
    children.map(async ({ child, label }) => ({
      exitCode: await child.exited,
      label,
    })),
  );
  stopChildren();
  await Promise.all(children.map(({ child }) => child.exited));
  console.log(`${first.label} stopped.`);
  process.exitCode = first.exitCode;
} finally {
  releaseRunnerLease();
}
