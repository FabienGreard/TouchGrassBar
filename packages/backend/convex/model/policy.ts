export const BACKEND_POLICY_VERSION = "backend-policy-v1";

type BackendPolicy = {
  authentication: {
    canaryAuthRowsPerModel: number;
    canaryLifetimeMs: number;
    canaryRelatedRowsPerTable: number;
    profilePreparationAttempts: number;
    profilePreparationWindowMs: number;
  };
  doomerboards: {
    globalResultLimit: number;
    globalScanRows: number;
    legacyCompatibilityRows: number;
    savedTokenmaxxers: number;
  };
  health: {
    maxPages: number;
    pageSize: number;
  };
  recovery: {
    attemptLifetimeMs: number;
    authFinalizationLeaseMs: number;
    commitGraceMs: number;
    failedAttemptReservationMs: number;
    failedAttempts: number;
    failedAttemptWindowMs: number;
    successfulTransfers: number;
    successfulTransferWindowMs: number;
  };
  synchronization: {
    maxActiveMacSegmentsPerProviderDay: number;
    maxProfileBackfillSnapshots: number;
    maxSnapshotsPerRequest: number;
    maxTransferDayCarryovers: number;
    profileBackfillDays: number;
    rateCapacity: number;
    ratePerMinute: number;
    retainedUsageDays: number;
  };
};

const minuteMs = 60 * 1_000;

export const backendPolicy = {
  authentication: {
    canaryAuthRowsPerModel: 16,
    canaryLifetimeMs: 30 * minuteMs,
    canaryRelatedRowsPerTable: 128,
    profilePreparationAttempts: 5,
    profilePreparationWindowMs: minuteMs,
  },
  doomerboards: {
    globalResultLimit: 100,
    globalScanRows: 640,
    legacyCompatibilityRows: 640,
    savedTokenmaxxers: 100,
  },
  health: {
    maxPages: 1_000,
    pageSize: 100,
  },
  recovery: {
    attemptLifetimeMs: 5 * minuteMs,
    authFinalizationLeaseMs: minuteMs,
    commitGraceMs: minuteMs,
    failedAttemptReservationMs: 15 * minuteMs,
    failedAttempts: 5,
    failedAttemptWindowMs: 15 * minuteMs,
    successfulTransfers: 3,
    successfulTransferWindowMs: 60 * minuteMs,
  },
  synchronization: {
    maxActiveMacSegmentsPerProviderDay: 73,
    maxProfileBackfillSnapshots: 60,
    maxSnapshotsPerRequest: 62,
    maxTransferDayCarryovers: 2,
    profileBackfillDays: 30,
    rateCapacity: 180,
    ratePerMinute: 60,
    retainedUsageDays: 60,
  },
} as const satisfies BackendPolicy;
