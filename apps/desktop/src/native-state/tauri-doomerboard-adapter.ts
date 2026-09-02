import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { REVISION_NOTICE_EVENT } from "@touchgrass/contracts";

import type { DoomerboardPort, DoomerboardPortOutcome } from "@/native-state/doomerboard-query";

type StopListening = () => void | Promise<void>;
type TauriDoomerboardBindings = {
  invoke: (command: string, args?: Record<string, unknown>) => Promise<unknown>;
  listen: (event: string, receive: (event: { payload: unknown }) => void) => Promise<StopListening>;
  onFocusChanged: (receive: (event: { payload: boolean }) => void) => Promise<StopListening>;
};

const defaultBindings: TauriDoomerboardBindings = {
  invoke: (command, args) => invoke<unknown>(command, args),
  listen: (event, receive) => listen<unknown>(event, receive),
  onFocusChanged: (receive) => getCurrentWindow().onFocusChanged(receive),
};

let nextDoomerboardReadSequence = 0;

function createDoomerboardReadId() {
  nextDoomerboardReadSequence += 1;
  return `${Date.now().toString(36)}-${nextDoomerboardReadSequence.toString(36)}`;
}

function unavailable<Value>(): DoomerboardPortOutcome<Value> {
  return {
    fault: { code: "doomerboard-unavailable" },
    ok: false,
  };
}

function stopSafely(stop: StopListening | null) {
  try {
    const cleanup = stop?.();
    if (cleanup !== undefined) {
      void cleanup.catch(() => {
        // Tauri cleanup failures are private transport details.
      });
    }
  } catch {
    // Tauri cleanup failures are private transport details.
  }
}

function millisecondsUntilNextUtcDay(now = new Date()) {
  const nextUtcDay = Date.UTC(now.getUTCFullYear(), now.getUTCMonth(), now.getUTCDate() + 1);
  return Math.max(nextUtcDay - now.getTime(), 1);
}

function createTauriDoomerboardAdapter(
  bindings: TauriDoomerboardBindings = defaultBindings,
): DoomerboardPort {
  return {
    add: async (profileKey, touchGrassId) => {
      try {
        return {
          ok: true,
          value: await bindings.invoke("add_tokenmaxxer", { profileKey, touchGrassId }),
        };
      } catch {
        return unavailable();
      }
    },
    read: async (profileKey, query, signal) => {
      if (signal?.aborted) return unavailable();
      const requestId = createDoomerboardReadId();
      const read = bindings.invoke("get_doomerboard", { profileKey, query, requestId });
      const cancelRead = () => {
        void bindings.invoke("cancel_doomerboard_read", { requestId }).catch(() => undefined);
      };
      signal?.addEventListener("abort", cancelRead, { once: true });
      if (signal?.aborted) cancelRead();
      try {
        return {
          ok: true,
          value: await read,
        };
      } catch {
        return unavailable();
      } finally {
        signal?.removeEventListener("abort", cancelRead);
      }
    },
    subscribeFocus: async (receive) => {
      try {
        const stop = await bindings.onFocusChanged(({ payload: focused }) => receive(focused));
        return {
          ok: true,
          value: () => stopSafely(stop),
        };
      } catch {
        return unavailable();
      }
    },
    subscribe: async (receive) => {
      let closed = false;
      let stopRevision: StopListening | null = null;
      let rolloverTimer: ReturnType<typeof setTimeout> | null = null;
      const stopAll = () => {
        closed = true;
        if (rolloverTimer !== null) clearTimeout(rolloverTimer);
        rolloverTimer = null;
        stopSafely(stopRevision);
        stopRevision = null;
      };
      const scheduleRollover = () => {
        if (closed) return;
        rolloverTimer = setTimeout(() => {
          rolloverTimer = null;
          if (closed) return;
          receive();
          scheduleRollover();
        }, millisecondsUntilNextUtcDay());
      };
      try {
        stopRevision = await bindings.listen(REVISION_NOTICE_EVENT, () => {
          if (!closed) receive();
        });
        scheduleRollover();
        return {
          ok: true,
          value: stopAll,
        };
      } catch {
        stopAll();
        return unavailable();
      }
    },
  };
}

export { createTauriDoomerboardAdapter };
export type { TauriDoomerboardBindings };
