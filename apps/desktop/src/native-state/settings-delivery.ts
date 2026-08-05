import {
  settingsNavigationRequestSchema,
  settingsStateSchema,
  type SettingsSection,
  type SettingsState,
} from "@touchgrass/contracts";

type SettingsPortFaultCode =
  | "launch-at-login-unavailable"
  | "navigation-stream-unavailable"
  | "settings-state-unavailable"
  | "surface-unavailable";

type SettingsPortOutcome<Value> =
  | { ok: true; value: Value }
  | { fault: { code: SettingsPortFaultCode }; ok: false };

type SettingsPort = {
  hide: () => Promise<SettingsPortOutcome<void>>;
  read: () => Promise<SettingsPortOutcome<unknown>>;
  setLaunchAtLogin: (enabled: boolean) => Promise<SettingsPortOutcome<unknown>>;
  subscribeNavigation: (
    receive: (payload: unknown) => void,
  ) => Promise<SettingsPortOutcome<() => void>>;
};

type SettingsDeliverySnapshot = {
  phase: "degraded" | "loading" | "ready";
  savingLaunchAtLogin: boolean;
  snapshot: SettingsState | null;
};

function createSettingsDelivery(port: SettingsPort) {
  let current: SettingsDeliverySnapshot = {
    phase: "loading",
    savingLaunchAtLogin: false,
    snapshot: null,
  };
  let readInFlight: Promise<void> | null = null;
  let saveInFlight: Promise<boolean> | null = null;
  let selectedSection: SettingsSection | null = null;
  const listeners = new Set<() => void>();

  const publish = (next: SettingsDeliverySnapshot) => {
    current = next;
    for (const listener of listeners) listener();
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
    publish({ phase: "ready", savingLaunchAtLogin, snapshot });
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
    selectedSection = request.data.section;
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
      const subscription = await port.subscribeNavigation(receiveNavigation);
      await read();
      return subscription.ok ? subscription.value : () => undefined;
    },
    getSnapshot: () => current,
    hide: async () => {
      await port.hide();
    },
    read,
    selectSection(section: SettingsSection) {
      selectedSection = section;
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
