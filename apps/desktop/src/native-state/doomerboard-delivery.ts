import {
  doomerboardViewSchema,
  type DoomerboardView,
} from "@touchgrass/contracts";

type DoomerboardPortOutcome<Value> =
  | { ok: true; value: Value }
  | { fault: { code: "doomerboard-unavailable" }; ok: false };

type DoomerboardPort = {
  read: () => Promise<DoomerboardPortOutcome<unknown>>;
  subscribe: (
    receive: () => void,
  ) => Promise<DoomerboardPortOutcome<() => void>>;
};

type DoomerboardDeliverySnapshot = {
  phase: "degraded" | "loading" | "ready";
  view: DoomerboardView | null;
};

function createDoomerboardDelivery(port: DoomerboardPort) {
  let current: DoomerboardDeliverySnapshot = {
    phase: "loading",
    view: null,
  };
  let readInFlight: Promise<void> | null = null;
  let readRequested = false;
  const listeners = new Set<() => void>();

  const publish = (next: DoomerboardDeliverySnapshot) => {
    current = next;
    for (const listener of listeners) listener();
  };

  const read = () => {
    readRequested = true;
    if (readInFlight !== null) return readInFlight;
    readInFlight = (async () => {
      while (readRequested) {
        readRequested = false;
        try {
          const outcome = await port.read();
          if (!outcome.ok) {
            publish({ ...current, phase: "degraded" });
            continue;
          }
          const parsed = doomerboardViewSchema.safeParse(outcome.value);
          if (!parsed.success) {
            publish({ ...current, phase: "degraded" });
            continue;
          }
          publish({ phase: "ready", view: parsed.data });
        } catch {
          publish({ ...current, phase: "degraded" });
        }
      }
    })().finally(() => {
      readInFlight = null;
    });
    return readInFlight;
  };

  return {
    async activate() {
      try {
        const subscription = await port.subscribe(() => void read());
        await read();
        if (!subscription.ok) {
          publish({ ...current, phase: "degraded" });
          return () => undefined;
        }
        return subscription.value;
      } catch {
        publish({ ...current, phase: "degraded" });
        return () => undefined;
      }
    },
    getSnapshot: () => current,
    read,
    subscribe(listener: () => void) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
  };
}

export { createDoomerboardDelivery };
export type {
  DoomerboardDeliverySnapshot,
  DoomerboardPort,
  DoomerboardPortOutcome,
};
