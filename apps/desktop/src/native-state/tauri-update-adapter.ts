import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { UPDATE_STATE_CHANGED_EVENT } from "@touchgrass/contracts";

import type {
  UpdatePort,
  UpdatePortFaultCode,
  UpdatePortOutcome,
} from "@/native-state/update-delivery";

type TauriUpdateBindings = {
  invoke: (
    command: string,
    args?: Record<string, unknown>,
  ) => Promise<unknown>;
  listen: (event: string, receive: () => void) => Promise<() => void>;
};

const defaultBindings: TauriUpdateBindings = {
  invoke: (command, args) => invoke<unknown>(command, args),
  listen: (event, receive) => listen(event, receive),
};

async function closedInvoke(
  bindings: TauriUpdateBindings,
  command: string,
  fault: UpdatePortFaultCode,
  args?: Record<string, unknown>,
): Promise<UpdatePortOutcome<unknown>> {
  try {
    return {
      ok: true,
      value:
        args === undefined
          ? await bindings.invoke(command)
          : await bindings.invoke(command, args),
    };
  } catch {
    return { fault: { code: fault }, ok: false };
  }
}

function createTauriUpdateAdapter(
  bindings: TauriUpdateBindings = defaultBindings,
): UpdatePort {
  return {
    check: () =>
      closedInvoke(bindings, "check_for_updates", "update-check-unavailable"),
    install: () =>
      closedInvoke(bindings, "install_update", "update-install-unavailable"),
    openLatestDmg: async () => {
      const outcome = await closedInvoke(
        bindings,
        "open_latest_dmg",
        "update-download-unavailable",
      );
      return outcome.ok ? { ok: true, value: undefined } : outcome;
    },
    openSource: async () => {
      const outcome = await closedInvoke(
        bindings,
        "open_source_repository",
        "update-source-unavailable",
      );
      return outcome.ok ? { ok: true, value: undefined } : outcome;
    },
    read: () =>
      closedInvoke(bindings, "get_update_state", "update-state-unavailable"),
    retry: () =>
      closedInvoke(bindings, "retry_update", "update-retry-unavailable"),
    setAutomaticChecks: (enabled) =>
      closedInvoke(
        bindings,
        "set_automatic_update_checks",
        "update-preference-unavailable",
        { enabled },
      ),
    subscribe: async (receive) => {
      try {
        const stop = await bindings.listen(UPDATE_STATE_CHANGED_EVENT, receive);
        return { ok: true, value: stop };
      } catch {
        return {
          fault: { code: "update-state-stream-unavailable" },
          ok: false,
        };
      }
    },
  };
}

export { createTauriUpdateAdapter };
export type { TauriUpdateBindings };
