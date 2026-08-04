import type { SanitizedDesktopState } from "@touchgrass/contracts";
import {
  refreshReceiptSchema,
  revisionNoticeSchema,
  sanitizedDesktopStateSchema,
} from "@touchgrass/contracts";

type StopListening = () => void;
const REFRESH_ACKNOWLEDGEMENT_TIMEOUT_MS = 2_000;
const SUBSCRIPTION_HANDSHAKE_TIMEOUT_MS = 1_000;

export type SanitizedDesktopStateInvalidation =
  { kind: "revision"; notice: unknown } | { kind: "surface-resumed" };

export type SanitizedDesktopStatePortFaultCode =
  | "snapshot-unavailable"
  | "refresh-unavailable"
  | "invalidation-stream-unavailable";

export type SanitizedDesktopStatePortOutcome<
  Value,
  Code extends SanitizedDesktopStatePortFaultCode = SanitizedDesktopStatePortFaultCode,
> =
  | { ok: true; value: Value }
  | { ok: false; fault: { code: Code } };

export type SanitizedDesktopStatePort = {
  readSnapshot: () => Promise<
    SanitizedDesktopStatePortOutcome<unknown, "snapshot-unavailable">
  >;
  requestRefresh: () => Promise<
    SanitizedDesktopStatePortOutcome<unknown, "refresh-unavailable">
  >;
  subscribeToInvalidations: (
    receive: (invalidation: SanitizedDesktopStateInvalidation) => void,
  ) => Promise<
    SanitizedDesktopStatePortOutcome<
      StopListening,
      "invalidation-stream-unavailable"
    >
  >;
};

export type SanitizedDesktopStateDeliveryView =
  | {
      phase: "loading";
      refreshing: boolean;
      snapshot: null;
    }
  | {
      phase: "ready";
      refreshing: boolean;
      snapshot: SanitizedDesktopState;
    }
  | {
      phase: "degraded";
      refreshing: boolean;
      snapshot: SanitizedDesktopState | null;
    };

export type SanitizedDesktopStateDelivery = {
  getSnapshot: () => SanitizedDesktopStateDeliveryView;
  requestRefresh: () => Promise<void>;
  subscribe: (notify: () => void) => StopListening;
};

const INITIAL_VIEW: SanitizedDesktopStateDeliveryView = {
  phase: "loading",
  refreshing: false,
  snapshot: null,
};

function sameView(
  left: SanitizedDesktopStateDeliveryView,
  right: SanitizedDesktopStateDeliveryView,
) {
  return (
    left.phase === right.phase &&
    left.refreshing === right.refreshing &&
    left.snapshot === right.snapshot
  );
}

export function createSanitizedDesktopStateDelivery(
  port: SanitizedDesktopStatePort,
): SanitizedDesktopStateDelivery {
  const subscribers = new Set<() => void>();
  let view = INITIAL_VIEW;
  let activation = 0;
  let subscriptionHealthy = false;
  let stopListening: StopListening | null = null;
  let activeRead: {
    activation: number;
    promise: Promise<void>;
    requested: boolean;
  } | null = null;
  let activeRefresh: Promise<void> | null = null;

  const publish = (nextView: SanitizedDesktopStateDeliveryView) => {
    if (sameView(view, nextView)) return;
    view = nextView;
    for (const subscriber of subscribers) subscriber();
  };

  const setRefreshing = (refreshing: boolean) => {
    if (view.refreshing === refreshing) return;
    publish({ ...view, refreshing });
  };

  const markDegraded = () => {
    publish({
      phase: "degraded",
      refreshing: view.refreshing,
      snapshot: view.snapshot,
    });
  };

  const publishSnapshot = (candidate: SanitizedDesktopState) => {
    const current = view.snapshot;
    const snapshot =
      current !== null && BigInt(candidate.revision) <= BigInt(current.revision)
        ? current
        : candidate;

    publish({
      phase: subscriptionHealthy ? "ready" : "degraded",
      refreshing: view.refreshing,
      snapshot,
    });
  };

  const readOnce = async (readActivation: number) => {
    try {
      const outcome = await port.readSnapshot();
      if (readActivation !== activation) return;
      if (!outcome.ok) {
        markDegraded();
        return;
      }
      publishSnapshot(sanitizedDesktopStateSchema.parse(outcome.value));
    } catch {
      if (readActivation === activation) markDegraded();
    }
  };

  const requestRead = () => {
    const readActivation = activation;
    if (activeRead?.activation === readActivation) {
      activeRead.requested = true;
      return activeRead.promise;
    }

    const cycle = {
      activation: readActivation,
      promise: Promise.resolve(),
      requested: true,
    };
    cycle.promise = (async () => {
      while (cycle.requested && cycle.activation === activation) {
        cycle.requested = false;
        await readOnce(cycle.activation);
      }
    })().finally(() => {
      if (activeRead !== cycle) return;
      activeRead = null;
      if (cycle.requested && cycle.activation === activation)
        void requestRead();
    });
    activeRead = cycle;

    return cycle.promise;
  };

  const receiveInvalidation = (
    invalidation: SanitizedDesktopStateInvalidation,
    invalidationActivation: number,
  ) => {
    if (invalidationActivation !== activation) return;
    if (invalidation.kind === "surface-resumed") {
      void requestRead();
      return;
    }

    const parsedNotice = revisionNoticeSchema.safeParse(invalidation.notice);
    if (!parsedNotice.success) {
      markDegraded();
      void requestRead();
      return;
    }

    if (
      view.snapshot !== null &&
      BigInt(parsedNotice.data.revision) <= BigInt(view.snapshot.revision)
    ) {
      return;
    }

    void requestRead();
  };

  const activate = async (nextActivation: number) => {
    let listening = false;
    const connection = Promise.resolve().then(() =>
      port.subscribeToInvalidations((invalidation) => {
        if (nextActivation !== activation || !listening) return;
        receiveInvalidation(invalidation, nextActivation);
      }),
    );
    let timeoutId: ReturnType<typeof setTimeout> | undefined;
    const timeout = new Promise<{ status: "timed-out" }>((resolve) => {
      timeoutId = setTimeout(
        () => resolve({ status: "timed-out" }),
        SUBSCRIPTION_HANDSHAKE_TIMEOUT_MS,
      );
    });

    const connect = async (unsubscribe: StopListening) => {
      if (nextActivation !== activation || subscribers.size === 0) {
        unsubscribe();
        return;
      }

      listening = true;
      stopListening = unsubscribe;
      subscriptionHealthy = true;
      await requestRead();
    };

    try {
      const result = await Promise.race([
        connection.then((outcome) => ({
          status: "settled" as const,
          outcome,
        })),
        timeout,
      ]);
      if (timeoutId !== undefined) clearTimeout(timeoutId);

      if (result.status === "timed-out") {
        void connection
          .then((outcome) => {
            if (outcome.ok) return connect(outcome.value);
          })
          .catch(() => undefined);
        if (nextActivation !== activation || subscribers.size === 0) return;
        subscriptionHealthy = false;
        markDegraded();
        await requestRead();
        return;
      }

      if (!result.outcome.ok) {
        if (nextActivation !== activation || subscribers.size === 0) return;
        subscriptionHealthy = false;
        markDegraded();
        await requestRead();
        return;
      }

      await connect(result.outcome.value);
    } catch {
      if (timeoutId !== undefined) clearTimeout(timeoutId);
      if (nextActivation !== activation || subscribers.size === 0) return;
      subscriptionHealthy = false;
      markDegraded();
      await requestRead();
    }
  };

  const subscribe = (notify: () => void) => {
    subscribers.add(notify);
    if (subscribers.size === 1) {
      const nextActivation = ++activation;
      if (activeRefresh !== null) setRefreshing(true);
      void activate(nextActivation);
    }

    let subscribed = true;
    return () => {
      if (!subscribed) return;
      subscribed = false;
      subscribers.delete(notify);
      if (subscribers.size !== 0) return;

      activation += 1;
      subscriptionHealthy = false;
      if (view.phase === "ready") {
        view = {
          phase: "degraded",
          refreshing: false,
          snapshot: view.snapshot,
        };
      } else if (view.refreshing) {
        view = { ...view, refreshing: false };
      }
      const unsubscribe = stopListening;
      stopListening = null;
      unsubscribe?.();
    };
  };

  const requestRefresh = () => {
    if (subscribers.size === 0) return Promise.resolve();
    if (activeRefresh !== null) {
      setRefreshing(true);
      return activeRefresh;
    }

    setRefreshing(true);
    const refresh = async () => {
      let timeoutId: ReturnType<typeof setTimeout> | undefined;
      try {
        const request = Promise.resolve().then(() => port.requestRefresh());
        const timeout = new Promise<{ status: "timed-out" }>((resolve) => {
          timeoutId = setTimeout(
            () => resolve({ status: "timed-out" }),
            REFRESH_ACKNOWLEDGEMENT_TIMEOUT_MS,
          );
        });
        const result = await Promise.race([
          request.then((outcome) => ({
            status: "settled" as const,
            outcome,
          })),
          timeout,
        ]);
        if (subscribers.size > 0) {
          if (result.status === "timed-out") {
            markDegraded();
          } else if (!result.outcome.ok) markDegraded();
          else {
            const receipt = refreshReceiptSchema.parse(result.outcome.value);
            if (receipt.accepted) void requestRead();
            else markDegraded();
          }
        }
      } catch {
        if (subscribers.size > 0) markDegraded();
      } finally {
        if (timeoutId !== undefined) clearTimeout(timeoutId);
        if (subscribers.size > 0) setRefreshing(false);
        else if (view.refreshing) view = { ...view, refreshing: false };
      }
    };

    let trackedRefresh: Promise<void>;
    trackedRefresh = refresh().finally(() => {
      if (activeRefresh === trackedRefresh) activeRefresh = null;
    });
    activeRefresh = trackedRefresh;

    return trackedRefresh;
  };

  return {
    getSnapshot: () => view,
    requestRefresh,
    subscribe,
  };
}
