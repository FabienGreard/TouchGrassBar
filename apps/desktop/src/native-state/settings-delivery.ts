import {
  settingsNavigationRequestSchema,
  settingsStateSchema,
  type CodingProvider,
  type SettingsSection,
  type SettingsState,
} from "@touchgrass/contracts";

import type { ProfileRecoveryCredentials } from "@/components/dialogs/recovery-dialog";

type SettingsPortFaultCode =
  | "display-name-update-unavailable"
  | "launch-at-login-unavailable"
  | "navigation-stream-unavailable"
  | "profile-recovery-unavailable"
  | "provider-setting-unavailable"
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
  recoverProfile: (credentials: ProfileRecoveryCredentials) => Promise<SettingsPortOutcome<void>>;
  revealRecoveryKey: () => Promise<SettingsPortOutcome<string>>;
  selectSection: (section: SettingsSection) => Promise<SettingsPortOutcome<void>>;
  setLaunchAtLogin: (enabled: boolean) => Promise<SettingsPortOutcome<unknown>>;
  updateDisplayName: (displayName: string) => Promise<SettingsPortOutcome<unknown>>;
  setProviderEnabled: (
    provider: CodingProvider,
    enabled: boolean,
  ) => Promise<SettingsPortOutcome<unknown>>;
  subscribeNavigation: (
    receive: (payload: unknown) => void,
  ) => Promise<SettingsPortOutcome<() => void>>;
  subscribeRecoveryClear: (receive: () => void) => Promise<SettingsPortOutcome<() => void>>;
};

type SettingsDeliverySnapshot = {
  phase: "degraded" | "loading" | "ready";
  recoveryFailed: boolean;
  recoveryKey: string | null;
  revealingRecoveryKey: boolean;
  savingLaunchAtLogin: boolean;
  savingProviders: readonly CodingProvider[];
  snapshot: SettingsState | null;
};

function createSettingsDelivery(port: SettingsPort) {
  let activationRevision = 0;
  let current: SettingsDeliverySnapshot = {
    phase: "loading",
    recoveryFailed: false,
    recoveryKey: null,
    revealingRecoveryKey: false,
    savingLaunchAtLogin: false,
    savingProviders: [],
    snapshot: null,
  };
  let readInFlight: Promise<void> | null = null;
  let recoveryClearAvailable = false;
  let recoveryInFlight: Promise<boolean> | null = null;
  let revealInFlight: Promise<boolean> | null = null;
  let recoveryRevision = 0;
  let saveInFlight: Promise<boolean> | null = null;
  let displayNameSaveInFlight: Promise<boolean> | null = null;
  let optimisticDisplayName: string | null = null;
  let providerConfirmationRevision = 0;
  const providerConfirmationRevisions = new Map<CodingProvider, number>();
  const providerSavesInFlight = new Map<CodingProvider, Promise<boolean>>();
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

  const accept = (
    value: unknown,
    savingLaunchAtLogin = false,
    observedProviderRevision = providerConfirmationRevision,
  ) => {
    const parsed = settingsStateSchema.safeParse(value);
    if (!parsed.success) {
      publish({ ...current, phase: "degraded", savingLaunchAtLogin });
      return false;
    }
    const section = selectedSection ?? parsed.data.section;
    selectedSection = section;
    const providers = parsed.data.providers.map((provider) => {
      const confirmationRevision = providerConfirmationRevisions.get(provider.provider) ?? 0;
      if (confirmationRevision <= observedProviderRevision) return provider;
      const lastConfirmed = current.snapshot?.providers.find(
        (item) => item.provider === provider.provider,
      );
      return lastConfirmed === undefined
        ? provider
        : { ...provider, enabled: lastConfirmed.enabled };
    });
    const snapshot = {
      ...parsed.data,
      ...(optimisticDisplayName === null ? {} : { displayName: optimisticDisplayName }),
      providers,
      recoveryKeySuffix: recoveryClearAvailable ? parsed.data.recoveryKeySuffix : null,
      section,
    };
    const previousRecoveryContext =
      current.snapshot?.profileProvisioning === "ready"
        ? [current.snapshot.touchGrassId, current.snapshot.recoveryKeySuffix].join(":")
        : null;
    const nextRecoveryContext =
      snapshot.profileProvisioning === "ready"
        ? [snapshot.touchGrassId, snapshot.recoveryKeySuffix].join(":")
        : null;
    if (current.snapshot !== null && previousRecoveryContext !== nextRecoveryContext) {
      clearRecoveryKey();
    }
    publish({
      phase: "ready",
      recoveryFailed: current.recoveryFailed,
      recoveryKey: current.recoveryKey,
      revealingRecoveryKey: current.revealingRecoveryKey,
      savingLaunchAtLogin,
      savingProviders: current.savingProviders,
      snapshot,
    });
    return true;
  };

  const publishDisplayName = (displayName: SettingsState["displayName"]) => {
    if (current.snapshot === null) return;
    const snapshot = { ...current.snapshot };
    if (displayName === undefined) {
      delete snapshot.displayName;
    } else {
      snapshot.displayName = displayName;
    }
    publish({ ...current, snapshot });
  };

  const read = () => {
    if (readInFlight !== null) return readInFlight;
    const observedProviderRevision = providerConfirmationRevision;
    readInFlight = (async () => {
      const outcome = await port.read();
      if (!outcome.ok) {
        publish({ ...current, phase: "degraded", savingLaunchAtLogin: false });
        return;
      }
      accept(outcome.value, false, observedProviderRevision);
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
      const revision = ++activationRevision;
      const [navigation, recoveryClear] = await Promise.all([
        port.subscribeNavigation(receiveNavigation),
        port.subscribeRecoveryClear(clearRecoveryKey),
      ]);
      const stopSubscriptions = () => {
        if (navigation.ok) navigation.value();
        if (recoveryClear.ok) recoveryClear.value();
      };
      if (revision !== activationRevision) {
        stopSubscriptions();
        return () => undefined;
      }
      recoveryClearAvailable = recoveryClear.ok;
      await read();
      if (revision !== activationRevision) {
        stopSubscriptions();
        return () => undefined;
      }
      if (!recoveryClearAvailable) {
        publish({ ...current, phase: "degraded" });
      }
      return () => {
        if (revision === activationRevision) {
          activationRevision += 1;
          recoveryClearAvailable = false;
          clearRecoveryKey();
        }
        stopSubscriptions();
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
    recoverProfile(credentials: ProfileRecoveryCredentials) {
      if (recoveryInFlight !== null) return recoveryInFlight;
      clearRecoveryKey();
      publish({ ...current, recoveryFailed: false });
      recoveryInFlight = (async () => {
        const recovered = await port.recoverProfile(credentials);
        if (!recovered.ok) {
          publish({ ...current, phase: "degraded", recoveryFailed: true });
          return false;
        }
        const state = await port.read();
        const accepted = state.ok && accept(state.value);
        if (!accepted) publish({ ...current, recoveryFailed: true });
        return accepted;
      })().finally(() => {
        recoveryInFlight = null;
      });
      return recoveryInFlight;
    },
    revealRecoveryKey() {
      if (!recoveryClearAvailable) return Promise.resolve(false);
      if (revealInFlight !== null) return revealInFlight;
      const revision = ++recoveryRevision;
      publish({ ...current, revealingRecoveryKey: true });
      revealInFlight = (async () => {
        if (!(await sectionSelection)) {
          publish({ ...current, revealingRecoveryKey: false });
          return false;
        }
        const outcome = await port.revealRecoveryKey();
        const recoveryKey = outcome.ok && revision === recoveryRevision ? outcome.value : null;
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
      const observedProviderRevision = providerConfirmationRevision;
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
        return accept(outcome.value, false, observedProviderRevision);
      })().finally(() => {
        saveInFlight = null;
      });
      return saveInFlight;
    },
    updateDisplayName(displayName: string) {
      if (displayNameSaveInFlight !== null) return displayNameSaveInFlight;
      const previousDisplayName = current.snapshot?.displayName;
      optimisticDisplayName = displayName;
      publishDisplayName(displayName);
      displayNameSaveInFlight = (async () => {
        try {
          const outcome = await port.updateDisplayName(displayName);
          optimisticDisplayName = null;
          if (outcome.ok && accept(outcome.value)) return true;
        } catch {
          optimisticDisplayName = null;
        }
        publishDisplayName(previousDisplayName);
        return false;
      })().finally(() => {
        displayNameSaveInFlight = null;
      });
      return displayNameSaveInFlight;
    },
    setProviderEnabled(provider: CodingProvider, enabled: boolean) {
      const existing = providerSavesInFlight.get(provider);
      if (existing !== undefined) return existing;

      publish({
        ...current,
        savingProviders: [...current.savingProviders, provider],
      });
      const save = (async () => {
        const outcome = await port.setProviderEnabled(provider, enabled);
        if (!outcome.ok) {
          publish({ ...current, phase: "degraded" });
          return false;
        }
        const parsed = settingsStateSchema.safeParse(outcome.value);
        const confirmedProvider = parsed.success
          ? parsed.data.providers.find((item) => item.provider === provider)
          : undefined;
        if (
          !parsed.success ||
          confirmedProvider === undefined ||
          confirmedProvider.enabled !== enabled
        ) {
          publish({ ...current, phase: "degraded" });
          return false;
        }

        if (
          current.snapshot !== null &&
          !current.snapshot.providers.some((item) => item.provider === provider)
        ) {
          publish({ ...current, phase: "degraded" });
          return false;
        }
        providerConfirmationRevision += 1;
        providerConfirmationRevisions.set(provider, providerConfirmationRevision);
        if (current.snapshot === null) {
          return accept(parsed.data);
        }
        publish({
          ...current,
          phase: "ready",
          snapshot: {
            ...current.snapshot,
            providers: current.snapshot.providers.map((item) =>
              item.provider === provider ? confirmedProvider : item,
            ),
          },
        });
        return true;
      })().finally(() => {
        providerSavesInFlight.delete(provider);
        if (current.savingProviders.includes(provider)) {
          publish({
            ...current,
            savingProviders: current.savingProviders.filter((item) => item !== provider),
          });
        }
      });
      providerSavesInFlight.set(provider, save);
      return save;
    },
    subscribe(listener: () => void) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
  };
}

export { createSettingsDelivery };
export type { SettingsDeliverySnapshot, SettingsPort, SettingsPortFaultCode, SettingsPortOutcome };
