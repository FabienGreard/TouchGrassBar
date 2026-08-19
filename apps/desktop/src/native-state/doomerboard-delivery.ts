import { doomerboardViewSchema, type DoomerboardView } from "@touchgrass/contracts";

type DoomerboardPortOutcome<Value> =
  | { ok: true; value: Value }
  | { fault: { code: "doomerboard-unavailable" }; ok: false };

type DoomerboardQuery = {
  audience: "global" | "mine";
  scope: "claude" | "codex" | "combined";
  windowDays: 1 | 7 | 30;
};

const defaultDoomerboardQuery: DoomerboardQuery = {
  audience: "global",
  scope: "combined",
  windowDays: 1,
};

type DoomerboardPort = {
  read: (query: DoomerboardQuery) => Promise<DoomerboardPortOutcome<unknown>>;
  subscribe: (receive: () => void) => Promise<DoomerboardPortOutcome<() => void>>;
};

type DoomerboardDeliverySnapshot = {
  phase: "degraded" | "loading" | "ready";
  view: DoomerboardView | null;
};

const sameQuery = (left: DoomerboardQuery, right: DoomerboardQuery) =>
  left.audience === right.audience &&
  left.scope === right.scope &&
  left.windowDays === right.windowDays;

function createDoomerboardDelivery(port: DoomerboardPort) {
  let current: DoomerboardDeliverySnapshot = {
    phase: "loading",
    view: null,
  };
  let readInFlight: Promise<void> | null = null;
  let readRequested = false;
  let query = defaultDoomerboardQuery;
  const listeners = new Set<() => void>();

  const publish = (next: DoomerboardDeliverySnapshot) => {
    current = next;
    for (const listener of listeners) listener();
  };

  const read = (nextQuery = query) => {
    if (!sameQuery(query, nextQuery)) {
      query = nextQuery;
      publish({ phase: "loading", view: null });
    }
    readRequested = true;
    if (readInFlight !== null) return readInFlight;
    readInFlight = (async () => {
      while (readRequested) {
        readRequested = false;
        const requestedQuery = query;
        try {
          const outcome = await port.read(requestedQuery);
          if (!sameQuery(requestedQuery, query)) {
            readRequested = true;
            continue;
          }
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
    select: (nextQuery: DoomerboardQuery) =>
      sameQuery(query, nextQuery) ? (readInFlight ?? Promise.resolve()) : read(nextQuery),
    subscribe(listener: () => void) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
  };
}

export { createDoomerboardDelivery, defaultDoomerboardQuery };
export type {
  DoomerboardDeliverySnapshot,
  DoomerboardPort,
  DoomerboardPortOutcome,
  DoomerboardQuery,
};
