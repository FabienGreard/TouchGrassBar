import { updateStateSchema, type UpdateState } from "@touchgrass/contracts";

type UpdatePortFaultCode =
  | "update-check-unavailable"
  | "update-download-unavailable"
  | "update-install-unavailable"
  | "update-preference-unavailable"
  | "update-retry-unavailable"
  | "update-source-unavailable"
  | "update-state-stream-unavailable"
  | "update-state-unavailable";

type UpdatePortOutcome<Value> =
  | { ok: true; value: Value }
  | { fault: { code: UpdatePortFaultCode }; ok: false };

type UpdatePort = {
  check: () => Promise<UpdatePortOutcome<unknown>>;
  install: () => Promise<UpdatePortOutcome<unknown>>;
  openLatestDmg: () => Promise<UpdatePortOutcome<void>>;
  openSource: () => Promise<UpdatePortOutcome<void>>;
  read: () => Promise<UpdatePortOutcome<unknown>>;
  retry: () => Promise<UpdatePortOutcome<unknown>>;
  setAutomaticChecks: (enabled: boolean) => Promise<UpdatePortOutcome<unknown>>;
  subscribe: (receive: () => void) => Promise<UpdatePortOutcome<() => void>>;
};

type UpdateDeliverySnapshot = {
  pendingAction: UpdatePendingAction | null;
  phase: "degraded" | "loading" | "ready";
  state: UpdateState | null;
};

type UpdatePendingAction = "check" | "install" | "retry";

function createUpdateDelivery(port: UpdatePort) {
  let actionInFlight: Promise<boolean> | null = null;
  let actionRequestRevision = 0;
  let current: UpdateDeliverySnapshot = { pendingAction: null, phase: "loading", state: null };
  let readInFlight: Promise<void> | null = null;
  let readRequested = false;
  const listeners = new Set<() => void>();

  const publish = (next: UpdateDeliverySnapshot) => {
    current = next;
    for (const listener of listeners) listener();
  };

  const accept = (value: unknown, pendingAction = current.pendingAction) => {
    const parsed = updateStateSchema.safeParse(value);
    if (!parsed.success) {
      publish({ ...current, pendingAction, phase: "degraded" });
      return false;
    }
    publish({ pendingAction, phase: "ready", state: parsed.data });
    return true;
  };

  const read = () => {
    readRequested = true;
    if (readInFlight !== null) return readInFlight;
    readInFlight = (async () => {
      while (readRequested) {
        readRequested = false;
        const observedActionRequestRevision = actionRequestRevision;
        const outcome = await port.read();
        if (observedActionRequestRevision !== actionRequestRevision) {
          readRequested = true;
        } else if (!outcome.ok) {
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

  const runAction = (
    pendingAction: UpdatePendingAction,
    action: () => Promise<UpdatePortOutcome<unknown>>,
  ) => {
    if (actionInFlight !== null) return actionInFlight;
    const status = current.state?.update.status;
    if (status === "checking" || status === "downloading" || status === "installing") {
      return Promise.resolve(true);
    }
    actionRequestRevision += 1;
    publish({ ...current, pendingAction });
    const request = (async () => {
      const outcome = await action();
      if (!outcome.ok) {
        publish({ ...current, pendingAction: null, phase: "degraded" });
        return false;
      }
      if (!updateStateSchema.safeParse(outcome.value).success) {
        publish({ ...current, pendingAction: null, phase: "degraded" });
        return false;
      }
      await read();
      return current.phase === "ready";
    })().finally(() => {
      if (actionInFlight === request) actionInFlight = null;
      if (current.pendingAction !== null) {
        publish({ ...current, pendingAction: null });
      }
    });
    actionInFlight = request;
    return request;
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
    check: () => runAction("check", port.check),
    getSnapshot: () => current,
    install: () => runAction("install", port.install),
    openLatestDmg: async () => (await port.openLatestDmg()).ok,
    openSource: async () => (await port.openSource()).ok,
    read,
    retry: () => runAction("retry", port.retry),
    setAutomaticChecks: (enabled: boolean) => run(() => port.setAutomaticChecks(enabled)),
    subscribe(listener: () => void) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
  };
}

export { createUpdateDelivery };
export type { UpdateDeliverySnapshot, UpdatePort, UpdatePortFaultCode, UpdatePortOutcome };
