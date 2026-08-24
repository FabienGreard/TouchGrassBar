type HealthInspection = {
  canaryResidue: { markers: number };
  componentChecks: Record<string, boolean>;
  deviceMigration: {
    devices: number;
    missingCompletionFields: number;
  };
  doomerboardInvariant: {
    aggregateEntries: number;
    extraEntries: number;
    invalidEntries: number;
    mismatchedEntries: number;
    missingEntries: number;
    publicScores: number;
  };
  requiredEnvironment: Record<string, boolean>;
  productionDeployment: string;
};

type FunctionLog = {
  error?: string | null;
  identifier: string;
  kind: "Completion" | "Progress";
  timestamp: number;
};

type ProductionHealthInput = {
  completedAt: string;
  expectedDeploymentName: string;
  inspection: HealthInspection;
  logs: FunctionLog[];
  window: {
    completedAtMs: number;
    startedAtMs: number;
  };
};

function expectedAuthorityRejection(log: FunctionLog) {
  return (
    log.identifier === "sync:dailyUsage" &&
    typeof log.error === "string" &&
    log.error.includes("authority-rejected")
  );
}

export function productionHealthReceipt(input: ProductionHealthInput) {
  const logsInWindow = input.logs.filter((log) => {
    const timestampMs = log.timestamp * 1_000;
    return timestampMs >= input.window.startedAtMs && timestampMs <= input.window.completedAtMs;
  });
  const errors = logsInWindow.filter(
    (log) => log.kind === "Completion" && typeof log.error === "string" && log.error.length > 0,
  );
  const expectedAuthorityRejections = errors.filter(expectedAuthorityRejection).length;
  const unhandledErrors = errors.length - expectedAuthorityRejections;
  const invariant = input.inspection.doomerboardInvariant;
  const invariantPassed =
    invariant.extraEntries === 0 &&
    invariant.invalidEntries === 0 &&
    invariant.mismatchedEntries === 0 &&
    invariant.missingEntries === 0 &&
    invariant.aggregateEntries === invariant.publicScores;
  const passed =
    Object.keys(input.inspection.componentChecks).sort().join(",") ===
      "betterAuth,doomerboard,migrations,rateLimiter" &&
    Object.values(input.inspection.componentChecks).every(Boolean) &&
    input.inspection.productionDeployment === input.expectedDeploymentName &&
    Object.values(input.inspection.requiredEnvironment).every(Boolean) &&
    input.inspection.canaryResidue.markers === 0 &&
    input.inspection.deviceMigration.missingCompletionFields === 0 &&
    invariantPassed &&
    expectedAuthorityRejections === 1 &&
    unhandledErrors === 0;
  return {
    completedAt: input.completedAt,
    counts: {
      aggregateEntries: invariant.aggregateEntries,
      canaryMarkers: input.inspection.canaryResidue.markers,
      componentsPassed: Object.values(input.inspection.componentChecks).filter(Boolean).length,
      devices: input.inspection.deviceMigration.devices,
      expectedAuthorityRejections,
      publicScores: invariant.publicScores,
      unhandledErrors,
    },
    status: passed ? ("passed" as const) : ("failed" as const),
  };
}
