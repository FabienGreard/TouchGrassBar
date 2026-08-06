#!/usr/bin/env bun

import { developmentTarget } from "./development-environment";
import { resolveDevelopmentSigningConfiguration } from "./macos-development-signing";

if (process.argv.length > 2) {
  throw new Error(`Unknown argument(s): ${process.argv.slice(2).join(", ")}`);
}

const target = developmentTarget(Bun.env);
console.log(`Convex target: ${target}`);
console.log("Profile services: configured");
try {
  resolveDevelopmentSigningConfiguration("app.touchgrass.bar.dev", Bun.env);
  console.log("Development signing identity: valid");
  console.log("Development provisioning profile: valid");
} catch (error) {
  console.error(
    error instanceof Error
      ? error.message
      : "The development signing configuration is invalid.",
  );
  process.exitCode = 1;
}
