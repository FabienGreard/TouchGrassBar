import {
  updateStateSchema,
  type UpdateState,
} from "@touchgrass/contracts";

type UpdatePortFaultCode =
  | "update-check-unavailable"
  | "update-download-unavailable"
  | "update-install-unavailable"
  | "update-retry-unavailable"
  | "update-state-stream-unavailable"
  | "update-state-unavailable";

type UpdatePortOutcome<Value> =
  | { ok: true; value: Value }
  | { fault: { code: UpdatePortFaultCode }; ok: false };

type UpdatePort = {
  check: () => Promise<UpdatePortOutcome<unknown>>;
  install: () => Promise<UpdatePortOutcome<unknown>>;
  openLatestDmg: () => Promise<UpdatePortOutcome<void>>;
  read: () => Promise<UpdatePortOutcome<unknown>>;
  retry: () => Promise<UpdatePortOutcome<unknown>>;
  subscribe: (
    receive: () => void,
  ) => Promise<UpdatePortOutcome<() => void>>;
};

type UpdateDeliverySnapshot = {
  phase: "degraded" | "loading" | "ready";
  state: UpdateState | null;
};

function createUpdateDelivery(port: UpdatePort) {
  let current: UpdateDeliverySnapshot = { phase: "loading", state: null };
  let readInFlight: Promise<void> | null = null;
  let readRequested = false;
  const listeners = new Set<() => void>();

  const publish = (next: UpdateDeliverySnapshot) => {
    current = next;
    for (const listener of listeners) listener();
  };

  const accept = (value: unknown) => {
    const parsed = updateStateSchema.safeParse(value);
    if (!parsed.success) {
      publish({ ...current, phase: "degraded" });
      return false;
    }
    publish({ phase: "ready", state: parsed.data });
    return true;
  };

  const read = () => {
    readRequested = true;
    if (readInFlight !== null) return readInFlight;
    readInFlight = (async () => {
      while (readRequested) {
        readRequested = false;
        const outcome = await port.read();
        if (!outcome.ok) {
          publish({ ...current, phase: "degraded" });
        } else {
          accept(outcome.value);
        }
      }
    })().finally(() => {
      readInFlight = null;
    });
    return readInFlight;
  };

  const run = async (action: () => Promise<UpdatePortOutcome<unknown>>) => {
    const outcome = await action();
    if (!outcome.ok) {
      publish({ ...current, phase: "degraded" });
      return false;
    }
    return accept(outcome.value);
  };

  return {
    async activate() {
      const subscription = await port.subscribe(() => void read());
      await read();
      if (!subscription.ok) {
        publish({ ...current, phase: "degraded" });
        return () => undefined;
      }
      return subscription.value;
    },
    check: () => run(port.check),
    getSnapshot: () => current,
    install: () => run(port.install),
    openLatestDmg: async () => (await port.openLatestDmg()).ok,
    read,
    retry: () => run(port.retry),
    subscribe(listener: () => void) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
  };
}

export { createUpdateDelivery };
export type {
  UpdateDeliverySnapshot,
  UpdatePort,
  UpdatePortFaultCode,
  UpdatePortOutcome,
};
