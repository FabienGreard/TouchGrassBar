import {
  settingsNavigationRequestSchema,
  settingsStateSchema,
  type SettingsSection,
  type SettingsState,
} from "@touchgrass/contracts";

type SettingsPortFaultCode =
  | "launch-at-login-unavailable"
  | "navigation-stream-unavailable"
  | "recovery-clear-stream-unavailable"
  | "recovery-key-unavailable"
  | "settings-section-unavailable"
  | "settings-state-unavailable"
  | "surface-unavailable";

type SettingsPortOutcome<Value> =
  | { ok: true; value: Value }
  | { fault: { code: SettingsPortFaultCode }; ok: false };

type SettingsPort = {
  hide: () => Promise<SettingsPortOutcome<void>>;
  read: () => Promise<SettingsPortOutcome<unknown>>;
  revealRecoveryKey: () => Promise<SettingsPortOutcome<string>>;
  selectSection: (
    section: SettingsSection,
  ) => Promise<SettingsPortOutcome<void>>;
  setLaunchAtLogin: (enabled: boolean) => Promise<SettingsPortOutcome<unknown>>;
  subscribeNavigation: (
    receive: (payload: unknown) => void,
  ) => Promise<SettingsPortOutcome<() => void>>;
  subscribeRecoveryClear: (
    receive: () => void,
  ) => Promise<SettingsPortOutcome<() => void>>;
};

type SettingsDeliverySnapshot = {
  phase: "degraded" | "loading" | "ready";
  recoveryKey: string | null;
  revealingRecoveryKey: boolean;
  savingLaunchAtLogin: boolean;
  snapshot: SettingsState | null;
};

function createSettingsDelivery(port: SettingsPort) {
  let current: SettingsDeliverySnapshot = {
    phase: "loading",
    recoveryKey: null,
    revealingRecoveryKey: false,
    savingLaunchAtLogin: false,
    snapshot: null,
  };
  let readInFlight: Promise<void> | null = null;
  let revealInFlight: Promise<boolean> | null = null;
  let recoveryRevision = 0;
  let saveInFlight: Promise<boolean> | null = null;
  let sectionSelection = Promise.resolve(true);
  let sectionRevision = 0;
  let selectedSection: SettingsSection | null = null;
  const listeners = new Set<() => void>();

  const publish = (next: SettingsDeliverySnapshot) => {
    current = next;
    for (const listener of listeners) listener();
  };

  const clearRecoveryKey = () => {
    recoveryRevision += 1;
    if (current.recoveryKey !== null || current.revealingRecoveryKey) {
      publish({
        ...current,
        recoveryKey: null,
        revealingRecoveryKey: false,
      });
    }
  };

  const accept = (value: unknown, savingLaunchAtLogin = false) => {
    const parsed = settingsStateSchema.safeParse(value);
    if (!parsed.success) {
      publish({ ...current, phase: "degraded", savingLaunchAtLogin });
      return false;
    }
    const section = selectedSection ?? parsed.data.section;
    selectedSection = section;
    const snapshot = { ...parsed.data, section };
    const previousRecoveryContext =
      current.snapshot?.profileProvisioning === "ready"
        ? [
            current.snapshot.touchGrassId,
            current.snapshot.recoveryKeySuffix,
          ].join(":")
        : null;
    const nextRecoveryContext =
      snapshot.profileProvisioning === "ready"
        ? [snapshot.touchGrassId, snapshot.recoveryKeySuffix].join(":")
        : null;
    if (
      current.snapshot !== null &&
      previousRecoveryContext !== nextRecoveryContext
    ) {
      clearRecoveryKey();
    }
    publish({
      phase: "ready",
      recoveryKey: current.recoveryKey,
      revealingRecoveryKey: current.revealingRecoveryKey,
      savingLaunchAtLogin,
      snapshot,
    });
    return true;
  };

  const read = () => {
    if (readInFlight !== null) return readInFlight;
    readInFlight = (async () => {
      const outcome = await port.read();
      if (!outcome.ok) {
        publish({ ...current, phase: "degraded", savingLaunchAtLogin: false });
        return;
      }
      accept(outcome.value);
    })().finally(() => {
      readInFlight = null;
    });
    return readInFlight;
  };

  const receiveNavigation = (payload: unknown) => {
    const request = settingsNavigationRequestSchema.safeParse(payload);
    if (!request.success) return;
    sectionRevision += 1;
    selectedSection = request.data.section;
    if (request.data.section !== "profile") {
      clearRecoveryKey();
    }
    sectionSelection = Promise.resolve(true);
    if (current.snapshot !== null) {
      publish({
        ...current,
        snapshot: { ...current.snapshot, section: request.data.section },
      });
    }
    void read();
  };

  return {
    async activate() {
      const [navigation, recoveryClear] = await Promise.all([
        port.subscribeNavigation(receiveNavigation),
        port.subscribeRecoveryClear(clearRecoveryKey),
      ]);
      await read();
      return () => {
        if (navigation.ok) navigation.value();
        if (recoveryClear.ok) recoveryClear.value();
      };
    },
    getSnapshot: () => current,
    hide: async () => {
      clearRecoveryKey();
      await port.hide();
    },
    async hideRecoveryKey() {
      clearRecoveryKey();
      return true;
    },
    read,
    revealRecoveryKey() {
      if (revealInFlight !== null) return revealInFlight;
      const revision = ++recoveryRevision;
      publish({ ...current, revealingRecoveryKey: true });
      revealInFlight = (async () => {
        if (!(await sectionSelection)) {
          publish({ ...current, revealingRecoveryKey: false });
          return false;
        }
        const outcome = await port.revealRecoveryKey();
        const recoveryKey =
          outcome.ok && revision === recoveryRevision ? outcome.value : null;
        publish({
          ...current,
          recoveryKey,
          revealingRecoveryKey: false,
        });
        return recoveryKey !== null;
      })().finally(() => {
        revealInFlight = null;
      });
      return revealInFlight;
    },
    selectSection(section: SettingsSection) {
      const revision = ++sectionRevision;
      selectedSection = section;
      if (section !== "profile") {
        clearRecoveryKey();
      }
      sectionSelection = (async () => {
        const outcome = await port.selectSection(section);
        return outcome.ok && revision === sectionRevision;
      })();
      if (current.snapshot === null) {
        return;
      }
      publish({ ...current, snapshot: { ...current.snapshot, section } });
    },
    setLaunchAtLogin(enabled: boolean) {
      if (saveInFlight !== null) return saveInFlight;
      publish({ ...current, savingLaunchAtLogin: true });
      saveInFlight = (async () => {
        const outcome = await port.setLaunchAtLogin(enabled);
        if (!outcome.ok) {
          publish({
            ...current,
            phase: "degraded",
            savingLaunchAtLogin: false,
          });
          return false;
        }
        return accept(outcome.value);
      })().finally(() => {
        saveInFlight = null;
      });
      return saveInFlight;
    },
    subscribe(listener: () => void) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
  };
}

export { createSettingsDelivery };
export type {
  SettingsDeliverySnapshot,
  SettingsPort,
  SettingsPortFaultCode,
  SettingsPortOutcome,
};
