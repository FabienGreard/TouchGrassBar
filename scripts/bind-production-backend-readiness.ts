#!/usr/bin/env bun

import { writeFileSync } from "node:fs";
import { resolve } from "node:path";

import {
  DEPLOYMENT_BINDING_PATH,
  renderDeploymentBinding,
} from "./backend-readiness/deployment-binding";
import {
  preflightMatchesSource,
  readBackendReadinessPreflight,
} from "./backend-readiness/preflight";
import { productionConfiguration } from "./backend-readiness/production-configuration";
import { sourceBinding } from "./backend-readiness/source-binding";

const workspaceRoot = resolve(import.meta.dir, "..");
const configuration = productionConfiguration(Bun.env);
const binding = sourceBinding(workspaceRoot);
const preflightIndex = process.argv.indexOf("--preflight");
const preflightPath = preflightIndex === -1 ? undefined : process.argv[preflightIndex + 1];
if (!preflightPath || process.argv.length !== 4) {
  throw new Error("Usage: bun backend:readiness:bind --preflight <path>");
}
const preflight = readBackendReadinessPreflight(preflightPath);
if (!preflightMatchesSource(preflight, binding)) {
  throw new Error("Backend readiness requires a passed, exact-source local preflight");
}

writeFileSync(
  resolve(workspaceRoot, DEPLOYMENT_BINDING_PATH),
  renderDeploymentBinding(binding, configuration.deployment.name),
  { mode: 0o644 },
);

console.log(`Production backend bundle bound for ${configuration.deployment.name}.`);
