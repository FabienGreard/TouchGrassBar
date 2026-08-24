#!/usr/bin/env bun

import { randomInt } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";

import { runAuthenticatedCanary } from "./backend-readiness/canary";
import {
  DEPLOYMENT_BINDING_PATH,
  deploymentBindingMatches,
} from "./backend-readiness/deployment-binding";
import {
  buildBackendReadinessEvidence,
  type BackendBinding,
  type CheckReceipt,
} from "./backend-readiness/evidence";
import { productionHealthReceipt } from "./backend-readiness/health";
import {
  preflightMatchesSource,
  readBackendReadinessPreflight,
} from "./backend-readiness/preflight";
import { productionConfiguration } from "./backend-readiness/production-configuration";
import { productionHealthInputs } from "./backend-readiness/production-health-port";
import { productionCanaryPort } from "./backend-readiness/production-port";
import { sourceBinding } from "./backend-readiness/source-binding";

const CREDENTIAL_ALPHABET = "23456789ABCDEFGHJKMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

function argument(name: string) {
  const index = process.argv.indexOf(name);
  return index === -1 ? undefined : process.argv[index + 1];
}

function randomCredential(length: number) {
  return Array.from(
    { length },
    () => CREDENTIAL_ALPHABET[randomInt(0, CREDENTIAL_ALPHABET.length)],
  ).join("");
}

function failedReceipt(): CheckReceipt {
  return { completedAt: new Date().toISOString(), status: "failed" };
}

function unavailableBinding(): BackendBinding {
  return {
    boardKeyVersion: "unavailable",
    commit: "unavailable",
    lockHash: "unavailable",
    policyVersion: "unavailable",
    schemaHash: "unavailable",
  };
}

async function main() {
  const preflightPath = argument("--preflight");
  const outputPath = argument("--output");
  if (!preflightPath || !outputPath || process.argv.length !== 6) {
    throw new Error("Usage: bun backend:readiness:production --preflight <path> --output <path>");
  }
  const workspaceRoot = resolve(import.meta.dir, "..");
  const configuration = productionConfiguration(Bun.env);
  const currentSourceBinding = sourceBinding(workspaceRoot, {
    allowedDirtyPaths: [DEPLOYMENT_BINDING_PATH],
  });
  if (
    !deploymentBindingMatches(
      readFileSync(resolve(workspaceRoot, DEPLOYMENT_BINDING_PATH), "utf8"),
      currentSourceBinding,
      configuration.deployment.name,
    )
  ) {
    throw new Error("Backend readiness deployment binding is invalid");
  }
  const preflight = readBackendReadinessPreflight(resolve(preflightPath));
  if (!preflightMatchesSource(preflight, currentSourceBinding)) {
    throw new Error("Backend readiness requires a passed, exact-source local preflight");
  }

  const startedAtMs = Date.now();
  let authenticatedCanary: CheckReceipt = failedReceipt();
  try {
    const result = await runAuthenticatedCanary(productionCanaryPort(configuration), {
      now: Date.now,
      randomCredential,
    });
    authenticatedCanary = {
      completedAt: result.completedAt,
      counts: {
        aggregateEntriesRemoved: result.cleanup.aggregateEntriesRemoved,
        appRecordsRemoved: result.cleanup.appRecordsRemoved,
        authRecordsRemoved: result.cleanup.authRecordsRemoved,
        checksPassed: Object.values(result.checks).filter(Boolean).length,
        rateLimitKeysReset: result.cleanup.rateLimitKeysReset,
      },
      status: "passed",
    };
  } catch {
    authenticatedCanary = failedReceipt();
  }

  let productionHealth: CheckReceipt = failedReceipt();
  let runtimeBinding = unavailableBinding();
  try {
    const healthInputs = await productionHealthInputs(configuration, startedAtMs);
    runtimeBinding = healthInputs.inspection.runtimeBinding;
    const completedAtMs = healthInputs.completedAtMs;
    productionHealth = productionHealthReceipt({
      completedAt: new Date(completedAtMs).toISOString(),
      expectedDeploymentName: configuration.deployment.name,
      inspection: healthInputs.inspection,
      logs: healthInputs.logs,
      window: { completedAtMs, startedAtMs },
    });
  } catch {
    productionHealth = failedReceipt();
  }

  const evidence = buildBackendReadinessEvidence({
    checks: {
      authenticatedCanary,
      automatedSuite: preflight.checks.automatedSuite,
      migrationRehearsal: preflight.checks.migrationRehearsal,
      productionHealth,
    },
    deployment: {
      kind: configuration.deployment.kind,
      name: configuration.deployment.name,
      url: configuration.deployment.url,
    },
    generatedAt: new Date().toISOString(),
    runtimeBinding,
    sourceBinding: currentSourceBinding,
  });
  writeFileSync(resolve(outputPath), `${JSON.stringify(evidence, null, 2)}\n`, { mode: 0o600 });
  console.log(`Production backend readiness: ${evidence.readiness}`);
  if (evidence.readiness !== "canary-ready") process.exitCode = 1;
}

await main();
