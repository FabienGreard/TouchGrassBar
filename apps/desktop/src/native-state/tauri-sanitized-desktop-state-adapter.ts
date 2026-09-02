import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { REVISION_NOTICE_EVENT } from "@touchgrass/contracts";

import type { SanitizedDesktopStatePort } from "@/native-state/sanitized-desktop-state-delivery";

type StopListening = () => void | Promise<void>;
const SUBSCRIPTION_SETUP_TIMEOUT_MS = 1_000;

export type TauriSanitizedDesktopStateBindings = {
  invoke: (command: string) => Promise<unknown>;
  listen: (event: string, receive: (event: { payload: unknown }) => void) => Promise<StopListening>;
  onFocusChanged: (receive: (event: { payload: boolean }) => void) => Promise<StopListening>;
};

const defaultBindings: TauriSanitizedDesktopStateBindings = {
  invoke: (command) => invoke<unknown>(command),
  listen: (event, receive) => listen<unknown>(event, receive),
  onFocusChanged: (receive) => getCurrentWindow().onFocusChanged(receive),
};

function stopSafely(stopListening: StopListening) {
  try {
    const cleanup = stopListening();
    if (cleanup !== undefined) {
      void cleanup.catch(() => {
        // Tauri cleanup failures are private transport details.
      });
    }
  } catch {
    // Tauri cleanup failures are private transport details.
  }
}

function invalidationStreamUnavailable() {
  return {
    fault: { code: "invalidation-stream-unavailable" },
    ok: false,
  } as const;
}

export function createTauriSanitizedDesktopStateAdapter(
  bindings: TauriSanitizedDesktopStateBindings = defaultBindings,
): SanitizedDesktopStatePort {
  return {
    readSnapshot: async () => {
      try {
        return {
          ok: true,
          value: await bindings.invoke("get_sanitized_state"),
        };
      } catch {
        return {
          fault: { code: "snapshot-unavailable" },
          ok: false,
        };
      }
    },
    requestRefresh: async () => {
      try {
        return {
          ok: true,
          value: await bindings.invoke("request_refresh"),
        };
      } catch {
        return {
          fault: { code: "refresh-unavailable" },
          ok: false,
        };
      }
    },
    subscribeToInvalidations: async (receive) => {
      let closed = false;
      let stopRevisionNotices: StopListening | null = null;
      let stopFocusChanges: StopListening | null = null;
      const stopAll = () => {
        closed = true;
        if (stopFocusChanges !== null) stopSafely(stopFocusChanges);
        if (stopRevisionNotices !== null) stopSafely(stopRevisionNotices);
        stopFocusChanges = null;
        stopRevisionNotices = null;
      };

      const setup = async () => {
        try {
          stopRevisionNotices = await bindings.listen(REVISION_NOTICE_EVENT, (event) => {
            if (!closed) receive({ kind: "revision", notice: event.payload });
          });
          if (closed) {
            stopAll();
            return invalidationStreamUnavailable();
          }

          stopFocusChanges = await bindings.onFocusChanged(({ payload: focused }) => {
            if (!closed && focused) receive({ kind: "surface-resumed" });
          });
          if (closed) {
            stopAll();
            return invalidationStreamUnavailable();
          }

          return {
            ok: true,
            value: stopAll,
          } as const;
        } catch {
          stopAll();
          return invalidationStreamUnavailable();
        }
      };

      const setupResult = setup();
      let timeoutId: ReturnType<typeof setTimeout> | undefined;
      const timeout = new Promise<{ status: "timed-out" }>((resolve) => {
        timeoutId = setTimeout(
          () => resolve({ status: "timed-out" }),
          SUBSCRIPTION_SETUP_TIMEOUT_MS,
        );
      });
      const result = await Promise.race([
        setupResult.then((outcome) => ({
          status: "settled" as const,
          outcome,
        })),
        timeout,
      ]);
      if (timeoutId !== undefined) clearTimeout(timeoutId);

      if (result.status === "timed-out") {
        stopAll();
        void setupResult
          .then((outcome) => {
            if (outcome.ok) outcome.value();
          })
          .catch(() => undefined);
        return invalidationStreamUnavailable();
      }

      return result.outcome;
    },
  };
}
