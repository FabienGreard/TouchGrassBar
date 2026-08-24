import { invoke } from "@tauri-apps/api/core";

import type {
  BootstrapPort,
  BootstrapPortFaultCode,
  BootstrapPortOutcome,
} from "@/native-state/bootstrap-delivery";

type TauriBootstrapBindings = {
  invoke: (command: string, payload?: Record<string, unknown>) => Promise<unknown>;
};

const defaultBindings: TauriBootstrapBindings = {
  invoke: (command, payload) => invoke<unknown>(command, payload),
};

async function closedInvoke(
  bindings: TauriBootstrapBindings,
  command: string,
  fault: BootstrapPortFaultCode,
  payload?: Record<string, unknown>,
): Promise<BootstrapPortOutcome<unknown>> {
  try {
    return { ok: true, value: await bindings.invoke(command, payload) };
  } catch {
    return { fault: { code: fault }, ok: false };
  }
}

function createTauriBootstrapAdapter(
  bindings: TauriBootstrapBindings = defaultBindings,
): BootstrapPort {
  return {
    complete: (displayName) =>
      closedInvoke(bindings, "complete_bootstrap", "bootstrap-completion-unavailable", {
        displayName,
      }),
    hide: async () => {
      const outcome = await closedInvoke(bindings, "hide_surface", "surface-unavailable");
      return outcome.ok ? { ok: true, value: undefined } : outcome;
    },
    read: () => closedInvoke(bindings, "get_bootstrap_state", "bootstrap-state-unavailable"),
    recoverProfile: async (credentials) => {
      const outcome = await closedInvoke(
        bindings,
        "recover_profile",
        "profile-recovery-unavailable",
        {
          recoveryKey: credentials.recoveryKey,
          touchGrassId: credentials.touchGrassId,
        },
      );
      return outcome.ok ? { ok: true, value: undefined } : outcome;
    },
  };
}

export { createTauriBootstrapAdapter };
export type { TauriBootstrapBindings };
