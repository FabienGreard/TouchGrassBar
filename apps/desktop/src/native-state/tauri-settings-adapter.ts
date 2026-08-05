import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { SETTINGS_NAVIGATION_EVENT } from "@touchgrass/contracts";

import type {
  SettingsPort,
  SettingsPortFaultCode,
  SettingsPortOutcome,
} from "@/native-state/settings-delivery";

type TauriSettingsBindings = {
  invoke: (
    command: string,
    payload?: Record<string, unknown>,
  ) => Promise<unknown>;
  listen: (
    event: string,
    receive: (event: { payload: unknown }) => void,
  ) => Promise<() => void>;
};

const defaultBindings: TauriSettingsBindings = {
  invoke: (command, payload) => invoke<unknown>(command, payload),
  listen: (event, receive) => listen<unknown>(event, receive),
};

async function closedInvoke(
  bindings: TauriSettingsBindings,
  command: string,
  fault: SettingsPortFaultCode,
  payload?: Record<string, unknown>,
): Promise<SettingsPortOutcome<unknown>> {
  try {
    return { ok: true, value: await bindings.invoke(command, payload) };
  } catch {
    return { fault: { code: fault }, ok: false };
  }
}

function createTauriSettingsAdapter(
  bindings: TauriSettingsBindings = defaultBindings,
): SettingsPort {
  return {
    hide: async () => {
      const outcome = await closedInvoke(
        bindings,
        "hide_surface",
        "surface-unavailable",
      );
      return outcome.ok ? { ok: true, value: undefined } : outcome;
    },
    read: () =>
      closedInvoke(
        bindings,
        "get_settings_state",
        "settings-state-unavailable",
      ),
    requestRecoveryDisclosure: async () => {
      const outcome = await closedInvoke(
        bindings,
        "request_recovery_disclosure",
        "recovery-key-unavailable",
      );
      return outcome.ok ? { ok: true, value: undefined } : outcome;
    },
    revealRecoveryKey: async () => {
      const outcome = await closedInvoke(
        bindings,
        "reveal_recovery_key",
        "recovery-key-unavailable",
      );
      return outcome.ok ? { ok: true, value: undefined } : outcome;
    },
    selectSection: async (section) => {
      const outcome = await closedInvoke(
        bindings,
        "select_settings_section",
        "settings-section-unavailable",
        { section },
      );
      return outcome.ok ? { ok: true, value: undefined } : outcome;
    },
    setLaunchAtLogin: (enabled) =>
      closedInvoke(
        bindings,
        "set_launch_at_login",
        "launch-at-login-unavailable",
        { enabled },
      ),
    subscribeNavigation: async (receive) => {
      try {
        const stop = await bindings.listen(
          SETTINGS_NAVIGATION_EVENT,
          ({ payload }) => receive(payload),
        );
        return { ok: true, value: stop };
      } catch {
        return {
          fault: { code: "navigation-stream-unavailable" },
          ok: false,
        };
      }
    },
  };
}

export { createTauriSettingsAdapter };
export type { TauriSettingsBindings };
