import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { REVISION_NOTICE_EVENT } from "@touchgrass/contracts";

import type {
  DoomerboardPort,
  DoomerboardPortOutcome,
} from "@/native-state/doomerboard-delivery";

type StopListening = () => void;
type TauriDoomerboardBindings = {
  invoke: (command: string) => Promise<unknown>;
  listen: (
    event: string,
    receive: (event: { payload: unknown }) => void,
  ) => Promise<StopListening>;
  onFocusChanged: (
    receive: (event: { payload: boolean }) => void,
  ) => Promise<StopListening>;
};

const defaultBindings: TauriDoomerboardBindings = {
  invoke: (command) => invoke<unknown>(command),
  listen: (event, receive) => listen<unknown>(event, receive),
  onFocusChanged: (receive) => getCurrentWindow().onFocusChanged(receive),
};

function unavailable<Value>(): DoomerboardPortOutcome<Value> {
  return {
    fault: { code: "doomerboard-unavailable" },
    ok: false,
  };
}

function stopSafely(stop: StopListening | null) {
  try {
    stop?.();
  } catch {
    // Tauri cleanup failures are private transport details.
  }
}

function createTauriDoomerboardAdapter(
  bindings: TauriDoomerboardBindings = defaultBindings,
): DoomerboardPort {
  return {
    read: async () => {
      try {
        return {
          ok: true,
          value: await bindings.invoke("get_global_doomerboard"),
        };
      } catch {
        return unavailable();
      }
    },
    subscribe: async (receive) => {
      let stopRevision: StopListening | null = null;
      let stopFocus: StopListening | null = null;
      try {
        stopRevision = await bindings.listen(REVISION_NOTICE_EVENT, receive);
        stopFocus = await bindings.onFocusChanged(({ payload: focused }) => {
          if (focused) receive();
        });
        return {
          ok: true,
          value: () => {
            stopSafely(stopRevision);
            stopSafely(stopFocus);
          },
        };
      } catch {
        stopSafely(stopRevision);
        stopSafely(stopFocus);
        return unavailable();
      }
    },
  };
}

export { createTauriDoomerboardAdapter };
export type { TauriDoomerboardBindings };
