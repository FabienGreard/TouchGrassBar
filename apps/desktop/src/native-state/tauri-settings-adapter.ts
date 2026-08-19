import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  SETTINGS_NAVIGATION_EVENT,
  SETTINGS_RECOVERY_CLEAR_EVENT,
} from "@touchgrass/contracts";

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

const recoveryKeyPattern =
  /^[23456789ABCDEFGHJKMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz]{48}$/;

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
    recoverProfile: async () => {
      const outcome = await closedInvoke(
        bindings,
        "recover_profile",
        "profile-recovery-unavailable",
      );
      if (!outcome.ok || typeof outcome.value !== "boolean") {
        return {
          fault: { code: "profile-recovery-unavailable" },
          ok: false,
        };
      }
      return { ok: true, value: outcome.value };
    },
    revealRecoveryKey: async () => {
      const outcome = await closedInvoke(
        bindings,
        "reveal_recovery_key",
        "recovery-key-unavailable",
      );
      if (
        !outcome.ok ||
        typeof outcome.value !== "string" ||
        !recoveryKeyPattern.test(outcome.value)
      ) {
        return {
          fault: { code: "recovery-key-unavailable" },
          ok: false,
        };
      }
      return { ok: true, value: outcome.value };
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
    updateDisplayName: (displayName) =>
      closedInvoke(
        bindings,
        "update_profile_display_name",
        "display-name-update-unavailable",
        { displayName },
      ),
    setProviderEnabled: (provider, enabled) =>
      closedInvoke(
        bindings,
        "set_provider_enabled",
        "provider-setting-unavailable",
        { enabled, provider },
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
    subscribeRecoveryClear: async (receive) => {
      try {
        const stop = await bindings.listen(
          SETTINGS_RECOVERY_CLEAR_EVENT,
          () => receive(),
        );
        return { ok: true, value: stop };
      } catch {
        return {
          fault: { code: "recovery-clear-stream-unavailable" },
          ok: false,
        };
      }
    },
  };
}

export { createTauriSettingsAdapter };
export type { TauriSettingsBindings };
