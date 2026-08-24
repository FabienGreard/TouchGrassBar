import { readFileSync } from "node:fs";

import type { BackendBinding, CheckReceipt } from "./evidence";

const MAX_PREFLIGHT_BYTES = 64 * 1_024;

export type BackendReadinessPreflight = {
  checks: {
    automatedSuite: CheckReceipt;
    migrationRehearsal: CheckReceipt;
  };
  contractVersion: 1;
  generatedAt: string;
  sourceBinding: BackendBinding;
};

function record(value: unknown): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new Error("Backend readiness preflight is invalid");
  }
  return value as Record<string, unknown>;
}

function exactString(value: unknown, pattern: RegExp) {
  if (typeof value !== "string" || !pattern.test(value)) {
    throw new Error("Backend readiness preflight is invalid");
  }
  return value;
}

function receipt(value: unknown): CheckReceipt {
  const candidate = record(value);
  const completedAt = exactString(
    candidate.completedAt,
    /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$/u,
  );
  const status = candidate.status;
  if (status !== "failed" && status !== "passed" && status !== "skipped") {
    throw new Error("Backend readiness preflight is invalid");
  }
  if (candidate.counts !== undefined) {
    const counts = record(candidate.counts);
    if (
      !Object.values(counts).every((count) => Number.isSafeInteger(count) && Number(count) >= 0)
    ) {
      throw new Error("Backend readiness preflight is invalid");
    }
    return { completedAt, counts: counts as Record<string, number>, status };
  }
  return { completedAt, status };
}

function binding(value: unknown): BackendBinding {
  const candidate = record(value);
  return {
    boardKeyVersion: exactString(candidate.boardKeyVersion, /^tokens-v\d+$/u),
    commit: exactString(candidate.commit, /^[0-9a-f]{40}$/u),
    lockHash: exactString(candidate.lockHash, /^[0-9a-f]{64}$/u),
    policyVersion: exactString(candidate.policyVersion, /^backend-policy-v\d+$/u),
    schemaHash: exactString(candidate.schemaHash, /^[0-9a-f]{64}$/u),
  };
}

export function readBackendReadinessPreflight(path: string): BackendReadinessPreflight {
  const text = readFileSync(path, "utf8");
  if (text.length > MAX_PREFLIGHT_BYTES) {
    throw new Error("Backend readiness preflight is invalid");
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(text) as unknown;
  } catch {
    throw new Error("Backend readiness preflight is invalid");
  }
  const candidate = record(parsed);
  if (candidate.contractVersion !== 1) {
    throw new Error("Backend readiness preflight is invalid");
  }
  const checks = record(candidate.checks);
  return {
    checks: {
      automatedSuite: receipt(checks.automatedSuite),
      migrationRehearsal: receipt(checks.migrationRehearsal),
    },
    contractVersion: 1,
    generatedAt: exactString(
      candidate.generatedAt,
      /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$/u,
    ),
    sourceBinding: binding(candidate.sourceBinding),
  };
}

export function preflightMatchesSource(
  preflight: BackendReadinessPreflight,
  sourceBinding: BackendBinding,
) {
  return (
    preflight.checks.automatedSuite.status === "passed" &&
    preflight.checks.migrationRehearsal.status === "passed" &&
    preflight.sourceBinding.boardKeyVersion === sourceBinding.boardKeyVersion &&
    preflight.sourceBinding.commit === sourceBinding.commit &&
    preflight.sourceBinding.lockHash === sourceBinding.lockHash &&
    preflight.sourceBinding.policyVersion === sourceBinding.policyVersion &&
    preflight.sourceBinding.schemaHash === sourceBinding.schemaHash
  );
}
