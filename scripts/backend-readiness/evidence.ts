type CheckStatus = "failed" | "passed" | "skipped";

export type CheckReceipt = {
  completedAt: string;
  counts?: Record<string, number>;
  status: CheckStatus;
};

export type BackendBinding = {
  boardKeyVersion: string;
  commit: string;
  lockHash: string;
  policyVersion: string;
  schemaHash: string;
};

export type BackendReadinessChecks = {
  authenticatedCanary: CheckReceipt;
  automatedSuite: CheckReceipt;
  migrationRehearsal: CheckReceipt;
  productionHealth: CheckReceipt;
};

export type BackendReadinessInput = {
  checks: BackendReadinessChecks;
  deployment: {
    kind: "production";
    name: string;
    url: string;
  };
  generatedAt: string;
  runtimeBinding: BackendBinding;
  sourceBinding: BackendBinding;
};

type StaleReason = keyof BackendBinding;

export type BackendReadinessEvidence = BackendReadinessInput & {
  contractVersion: 1;
  productionReadiness: "not-ready";
  readiness: "canary-ready" | "not-ready";
  staleReasons: StaleReason[];
  trafficEvidence: "canary-only";
};

const bindingFields = [
  "boardKeyVersion",
  "commit",
  "lockHash",
  "policyVersion",
  "schemaHash",
] as const satisfies readonly (keyof BackendBinding)[];

const mandatoryChecks = [
  "authenticatedCanary",
  "automatedSuite",
  "migrationRehearsal",
  "productionHealth",
] as const satisfies readonly (keyof BackendReadinessChecks)[];

export function buildBackendReadinessEvidence(
  input: BackendReadinessInput,
): BackendReadinessEvidence {
  const staleReasons = bindingFields.filter(
    (field) => input.sourceBinding[field] !== input.runtimeBinding[field],
  );
  const checksPassed = mandatoryChecks.every((name) => input.checks[name]?.status === "passed");
  return {
    ...input,
    contractVersion: 1,
    productionReadiness: "not-ready",
    readiness: staleReasons.length === 0 && checksPassed ? "canary-ready" : "not-ready",
    staleReasons,
    trafficEvidence: "canary-only",
  };
}
