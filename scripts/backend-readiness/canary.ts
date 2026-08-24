type CleanupResult = {
  aggregateEntriesRemoved: number;
  appRecordsRemoved: number;
  authRecordsRemoved: number;
  cleanupComplete: boolean;
  rateLimitKeysReset: number;
};

type UsageAcknowledgement = {
  outcome: "committed" | "conflict" | "idempotent" | "stale";
};

export type CanaryPort = {
  cleanup: (args: { displayName: string; touchGrassId: string }) => Promise<CleanupResult>;
  commitRecovery: (args: {
    currentRecoveryKey: string;
    installationCredential: string;
    newRecoveryKey: string;
    recoveryProof: string;
  }) => Promise<{ activeMacGeneration: number; authFinalized: boolean }>;
  ensureProfile: (args: {
    displayName: string;
    installationCredential: string;
    jwt: string;
    touchGrassId: string;
  }) => Promise<{ activeMacGeneration: number }>;
  exchangeSession: (session: string) => Promise<string>;
  globalRows: (args: {
    jwt: string;
    rankingDay: string;
  }) => Promise<Array<{ touchGrassId: string }>>;
  myTokenmaxxerRows: (args: {
    jwt: string;
    rankingDay: string;
  }) => Promise<{ rows: unknown[]; savedTokenmaxxerCount: number }>;
  prepareProfile: () => Promise<{ signupProof: string; touchGrassId: string }>;
  prepareRecovery: (args: {
    attemptId: string;
    recoveryKey: string;
    replacementRecoveryKey: string;
    touchGrassId: string;
  }) => Promise<{ recoveryProof: string }>;
  registerCanary: (args: { displayName: string; touchGrassId: string }) => Promise<void>;
  signIn: (args: { recoveryKey: string; touchGrassId: string }) => Promise<string>;
  signUp: (args: {
    displayName: string;
    recoveryKey: string;
    signupProof: string;
    touchGrassId: string;
  }) => Promise<void>;
  syncUsage: (args: {
    activeMacGeneration: number;
    installationCredential: string;
    jwt: string;
    observedAt: number;
    observedTokens: number;
    rankingDay: string;
    revision: number;
  }) => Promise<UsageAcknowledgement[]>;
};

type CanaryRuntime = {
  now: () => number;
  randomCredential: (length: number) => string;
};

export class AuthorityRejectedError extends Error {
  constructor() {
    super("Active Mac authority was rejected");
    this.name = "AuthorityRejectedError";
  }
}

function assertCanary(condition: unknown, check: string): asserts condition {
  if (!condition) throw new Error(`Authenticated canary ${check} failed`);
}

function onlyOutcome(
  acknowledgements: UsageAcknowledgement[],
  outcome: UsageAcknowledgement["outcome"],
) {
  return acknowledgements.length === 1 && acknowledgements[0]?.outcome === outcome;
}

export async function runAuthenticatedCanary(port: CanaryPort, runtime: CanaryRuntime) {
  const startedAt = new Date(runtime.now()).toISOString();
  const displayName = `Readiness Canary ${runtime.randomCredential(12)}`;
  const recoveryKey = runtime.randomCredential(48);
  const replacementRecoveryKey = runtime.randomCredential(48);
  const oldInstallationCredential = runtime.randomCredential(52);
  const newInstallationCredential = runtime.randomCredential(52);
  const attemptId = runtime.randomCredential(32);
  let touchGrassId: string | null = null;
  let cleanup: CleanupResult | null = null;
  let flowError: unknown = null;
  const checks = {
    cleanup: false,
    generatedCredentials: false,
    globalRead: false,
    identicalRetry: false,
    myTokenmaxxersRead: false,
    newMacSync: false,
    oldMacRejected: false,
    sessionExchange: false,
    synchronization: false,
    transfer: false,
  };

  try {
    const prepared = await port.prepareProfile();
    touchGrassId = prepared.touchGrassId;
    await port.registerCanary({ displayName, touchGrassId });
    await port.signUp({
      displayName,
      recoveryKey,
      signupProof: prepared.signupProof,
      touchGrassId,
    });
    checks.generatedCredentials = true;
    const oldSession = await port.signIn({ recoveryKey, touchGrassId });
    const oldJwt = await port.exchangeSession(oldSession);
    checks.sessionExchange = true;
    const activeMac = await port.ensureProfile({
      displayName,
      installationCredential: oldInstallationCredential,
      jwt: oldJwt,
      touchGrassId,
    });
    assertCanary(activeMac.activeMacGeneration === 1, "initial Active Mac claim");
    const observedAt = runtime.now();
    const initialRankingDay = new Date(observedAt).toISOString().slice(0, 10);
    const observedTokens = 1_000_000_000_000;
    const initialSync = {
      activeMacGeneration: 1,
      installationCredential: oldInstallationCredential,
      jwt: oldJwt,
      observedAt,
      observedTokens,
      rankingDay: initialRankingDay,
      revision: 1,
    };
    assertCanary(onlyOutcome(await port.syncUsage(initialSync), "committed"), "synchronization");
    checks.synchronization = true;
    assertCanary(onlyOutcome(await port.syncUsage(initialSync), "idempotent"), "identical retry");
    checks.identicalRetry = true;
    const initialGlobal = await port.globalRows({ jwt: oldJwt, rankingDay: initialRankingDay });
    assertCanary(
      initialGlobal.some((row) => row.touchGrassId === touchGrassId),
      "Global Doomerboard read",
    );
    checks.globalRead = true;
    const initialMyRows = await port.myTokenmaxxerRows({
      jwt: oldJwt,
      rankingDay: initialRankingDay,
    });
    assertCanary(
      initialMyRows.savedTokenmaxxerCount === 0 && initialMyRows.rows.length === 0,
      "My Tokenmaxxers read",
    );
    checks.myTokenmaxxersRead = true;

    const preparedRecovery = await port.prepareRecovery({
      attemptId,
      recoveryKey,
      replacementRecoveryKey,
      touchGrassId,
    });
    const transferred = await port.commitRecovery({
      currentRecoveryKey: recoveryKey,
      installationCredential: newInstallationCredential,
      newRecoveryKey: replacementRecoveryKey,
      recoveryProof: preparedRecovery.recoveryProof,
    });
    assertCanary(
      transferred.activeMacGeneration === 2 && transferred.authFinalized,
      "Active Mac transfer",
    );
    checks.transfer = true;
    const newSession = await port.signIn({ recoveryKey: replacementRecoveryKey, touchGrassId });
    const newJwt = await port.exchangeSession(newSession);
    const postTransferObservedAt = runtime.now();
    const postTransferRankingDay = new Date(postTransferObservedAt).toISOString().slice(0, 10);
    try {
      await port.syncUsage({
        ...initialSync,
        jwt: newJwt,
        observedAt: postTransferObservedAt,
        rankingDay: postTransferRankingDay,
        revision: 2,
      });
    } catch (error) {
      if (error instanceof AuthorityRejectedError) checks.oldMacRejected = true;
      else throw error;
    }
    assertCanary(checks.oldMacRejected, "old Active Mac rejection");
    assertCanary(
      onlyOutcome(
        await port.syncUsage({
          activeMacGeneration: 2,
          installationCredential: newInstallationCredential,
          jwt: newJwt,
          observedAt: postTransferObservedAt,
          observedTokens,
          rankingDay: postTransferRankingDay,
          revision: 1,
        }),
        "committed",
      ),
      "new Active Mac synchronization",
    );
    checks.newMacSync = true;
    assertCanary(
      (await port.globalRows({ jwt: newJwt, rankingDay: postTransferRankingDay })).some(
        (row) => row.touchGrassId === touchGrassId,
      ),
      "post-transfer Global Doomerboard read",
    );
    await port.myTokenmaxxerRows({ jwt: newJwt, rankingDay: postTransferRankingDay });
  } catch (error) {
    flowError = error;
  } finally {
    if (touchGrassId !== null) {
      try {
        cleanup = await port.cleanup({ displayName, touchGrassId });
        checks.cleanup = cleanup.cleanupComplete;
      } catch {
        checks.cleanup = false;
      }
    }
  }

  if (!checks.cleanup) throw new Error("Authenticated canary cleanup failed");
  if (flowError !== null) throw flowError;
  assertCanary(cleanup !== null, "cleanup receipt");
  return {
    checks,
    cleanup: {
      aggregateEntriesRemoved: cleanup.aggregateEntriesRemoved,
      appRecordsRemoved: cleanup.appRecordsRemoved,
      authRecordsRemoved: cleanup.authRecordsRemoved,
      rateLimitKeysReset: cleanup.rateLimitKeysReset,
    },
    completedAt: new Date(runtime.now()).toISOString(),
    startedAt,
  };
}
