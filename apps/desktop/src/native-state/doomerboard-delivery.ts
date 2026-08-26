import {
  ADD_TOKENMAXXER_CONTRACT_VERSION,
  addTokenmaxxerOutcomeSchema,
  doomerboardViewSchema,
  type AddTokenmaxxerOutcome,
  type DoomerboardView,
} from "@touchgrass/contracts";

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
  add: (touchGrassId: string) => Promise<DoomerboardPortOutcome<unknown>>;
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

const globalViewKey = (query: DoomerboardQuery) => `${query.scope}:${query.windowDays}`;

function createDoomerboardDelivery(port: DoomerboardPort) {
  let current: DoomerboardDeliverySnapshot = {
    phase: "loading",
    view: null,
  };
  const globalViews = new Map<string, DoomerboardView>();
  let readInFlight: Promise<void> | null = null;
  let readRequested = false;
  let query = defaultDoomerboardQuery;
  let viewQuery: DoomerboardQuery | null = null;
  const listeners = new Set<() => void>();

  const publish = (next: DoomerboardDeliverySnapshot) => {
    current = next;
    for (const listener of listeners) listener();
  };

  const publishFailure = (requestedQuery: DoomerboardQuery) => {
    publish({
      phase: "degraded",
      view: viewQuery !== null && sameQuery(viewQuery, requestedQuery) ? current.view : null,
    });
  };

  const publishCachedGlobalView = (requestedQuery: DoomerboardQuery) => {
    if (requestedQuery.audience !== "global") return;
    const cached = globalViews.get(globalViewKey(requestedQuery));
    if (cached === undefined) return;
    viewQuery = requestedQuery;
    publish({ phase: "ready", view: cached });
  };

  const read = (nextQuery = query) => {
    if (!sameQuery(query, nextQuery)) {
      query = nextQuery;
      publishCachedGlobalView(nextQuery);
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
            publishFailure(requestedQuery);
            continue;
          }
          const parsed = doomerboardViewSchema.safeParse(outcome.value);
          if (!parsed.success) {
            publishFailure(requestedQuery);
            continue;
          }
          if (parsed.data.status === "unavailable") {
            publishFailure(requestedQuery);
            continue;
          }
          if (requestedQuery.audience === "global") {
            globalViews.set(globalViewKey(requestedQuery), parsed.data);
          }
          viewQuery = requestedQuery;
          publish({ phase: "ready", view: parsed.data });
        } catch {
          if (!sameQuery(requestedQuery, query)) {
            readRequested = true;
            continue;
          }
          publishFailure(requestedQuery);
        }
      }
    })().finally(() => {
      readInFlight = null;
    });
    return readInFlight;
  };

  return {
    async addTokenmaxxer(touchGrassId: string): Promise<AddTokenmaxxerOutcome> {
      const unavailable = {
        contractVersion: ADD_TOKENMAXXER_CONTRACT_VERSION,
        status: "unavailable" as const,
      };
      try {
        const outcome = await port.add(touchGrassId);
        if (!outcome.ok) return unavailable;
        const parsed = addTokenmaxxerOutcomeSchema.safeParse(outcome.value);
        return parsed.success ? parsed.data : unavailable;
      } catch {
        return unavailable;
      }
    },
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
