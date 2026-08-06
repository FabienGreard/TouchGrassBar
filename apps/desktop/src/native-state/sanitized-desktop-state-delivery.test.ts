import type { SanitizedDesktopState } from "@touchgrass/contracts";
import { describe, expect, test, vi } from "vitest";

import {
  createSanitizedDesktopStateDelivery,
  type SanitizedDesktopStateInvalidation,
  type SanitizedDesktopStatePort,
  type SanitizedDesktopStatePortFaultCode,
  type SanitizedDesktopStatePortOutcome,
} from "@/native-state/sanitized-desktop-state-delivery";

function state(revision: string): SanitizedDesktopState {
  return {
    contractVersion: 3,
    generatedAt: "2026-08-03T00:00:00.000Z",
    profile: { status: "not-authorized" },
    revision,
    providers: [
      { availability: "unavailable", provider: "codex", quotaLanes: [] },
      { availability: "unavailable", provider: "claude", quotaLanes: [] },
    ],
    sync: { lastSuccessfulAt: null, status: "unavailable" },
    usage: {
      claude: {
        scanStatus: "unavailable",
        thirtyDays: { availability: "unavailable" },
        sevenDays: { availability: "unavailable" },
        today: { availability: "unavailable" },
      },
      codex: {
        scanStatus: "unavailable",
        thirtyDays: { availability: "unavailable" },
        sevenDays: { availability: "unavailable" },
        today: { availability: "unavailable" },
      },
    },
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((nextResolve) => {
    resolve = nextResolve;
  });
  return { promise, resolve };
}

function success<Value>(
  value: Value,
): SanitizedDesktopStatePortOutcome<Value, never> {
  return { ok: true, value };
}

function fault<Code extends SanitizedDesktopStatePortFaultCode>(
  code: Code,
): SanitizedDesktopStatePortOutcome<never, Code> {
  return { fault: { code }, ok: false };
}

function controlledAdapter(initialSnapshot: unknown = state("1")) {
  const reads: Array<unknown | Promise<unknown>> = [initialSnapshot];
  const refreshes: Array<unknown | Promise<unknown>> = [];
  let receiveInvalidation:
    ((invalidation: SanitizedDesktopStateInvalidation) => void) | null = null;
  let readCount = 0;
  let refreshCount = 0;
  let unsubscribeCount = 0;
  const operations: string[] = [];

  const port: SanitizedDesktopStatePort = {
    readSnapshot: async () => {
      operations.push("read");
      readCount += 1;
      return success(await reads.shift());
    },
    requestRefresh: async () => {
      refreshCount += 1;
      return success(await refreshes.shift());
    },
    subscribeToInvalidations: async (receive) => {
      operations.push("subscribe");
      receiveInvalidation = receive;
      return success(() => {
        unsubscribeCount += 1;
      });
    },
  };

  return {
    port,
    emit: (notice: unknown) =>
      receiveInvalidation?.({ kind: "revision", notice }),
    operations,
    queueRead: (snapshot: unknown | Promise<unknown>) => reads.push(snapshot),
    queueRefresh: (receipt: unknown | Promise<unknown>) =>
      refreshes.push(receipt),
    readCount: () => readCount,
    refreshCount: () => refreshCount,
    resume: () => receiveInvalidation?.({ kind: "surface-resumed" }),
    unsubscribeCount: () => unsubscribeCount,
  };
}

async function waitForRevision(
  delivery: ReturnType<typeof createSanitizedDesktopStateDelivery>,
  revision: string,
) {
  await waitFor(() => {
    expect(delivery.getSnapshot().snapshot?.revision).toBe(revision);
  });
}

async function waitFor(assertion: () => void) {
  let failure: unknown;
  for (let attempt = 0; attempt < 50; attempt += 1) {
    try {
      assertion();
      return;
    } catch (error) {
      failure = error;
      await Promise.resolve();
    }
  }
  throw failure;
}

describe("Sanitized Desktop State delivery", () => {
  test("subscribes before the initial validated snapshot read", async () => {
    const transport = controlledAdapter();
    const delivery = createSanitizedDesktopStateDelivery(transport.port);
    const unsubscribe = delivery.subscribe(() => undefined);

    await waitForRevision(delivery, "1");

    expect(transport.operations).toEqual(["subscribe", "read"]);
    expect(delivery.getSnapshot()).toMatchObject({
      phase: "ready",
      refreshing: false,
    });
    unsubscribe();
    expect(transport.unsubscribeCount()).toBe(1);
  });

  test("degrades without exposing a snapshot port fault", async () => {
    const delivery = createSanitizedDesktopStateDelivery({
      readSnapshot: () => Promise.resolve(fault("snapshot-unavailable")),
      requestRefresh: () => Promise.resolve(success({ accepted: true })),
      subscribeToInvalidations: () =>
        Promise.resolve(success(() => undefined)),
    });
    const unsubscribe = delivery.subscribe(() => undefined);

    await waitFor(() => expect(delivery.getSnapshot().phase).toBe("degraded"));
    expect(delivery.getSnapshot()).toEqual({
      phase: "degraded",
      refreshing: false,
      snapshot: null,
    });
    unsubscribe();
  });

  test("ends refresh feedback and degrades on a closed refresh fault", async () => {
    const delivery = createSanitizedDesktopStateDelivery({
      readSnapshot: () => Promise.resolve(success(state("1"))),
      requestRefresh: () => Promise.resolve(fault("refresh-unavailable")),
      subscribeToInvalidations: () =>
        Promise.resolve(success(() => undefined)),
    });
    const unsubscribe = delivery.subscribe(() => undefined);
    await waitForRevision(delivery, "1");

    await delivery.requestRefresh();

    expect(delivery.getSnapshot()).toMatchObject({
      phase: "degraded",
      refreshing: false,
      snapshot: { revision: "1" },
    });
    unsubscribe();
  });

  test("retains the last good snapshot across contract failures and recovers", async () => {
    const transport = controlledAdapter(state("9007199254740993"));
    const delivery = createSanitizedDesktopStateDelivery(transport.port);
    const unsubscribe = delivery.subscribe(() => undefined);
    await waitForRevision(delivery, "9007199254740993");

    transport.queueRead({ revision: "9007199254740994" });
    transport.emit({ revision: "9007199254740994" });
    await waitFor(() => {
      expect(delivery.getSnapshot().phase).toBe("degraded");
    });
    expect(delivery.getSnapshot().snapshot?.revision).toBe("9007199254740993");

    transport.queueRead(state("9007199254740994"));
    transport.emit({ malformed: true });
    await waitForRevision(delivery, "9007199254740994");
    expect(delivery.getSnapshot().phase).toBe("ready");
    unsubscribe();
  });

  test("never regresses when delayed reads return an older revision", async () => {
    const transport = controlledAdapter(state("9007199254740993"));
    const delivery = createSanitizedDesktopStateDelivery(transport.port);
    const unsubscribe = delivery.subscribe(() => undefined);
    await waitForRevision(delivery, "9007199254740993");

    transport.queueRead(state("9007199254740992"));
    transport.emit({ revision: "9007199254740994" });
    await waitFor(() => expect(transport.readCount()).toBe(2));

    expect(delivery.getSnapshot()).toMatchObject({
      phase: "ready",
      snapshot: { revision: "9007199254740993" },
    });
    unsubscribe();
  });

  test("catches up on surface resume when the final revision notice was missed", async () => {
    const transport = controlledAdapter();
    const delivery = createSanitizedDesktopStateDelivery(transport.port);
    const unsubscribe = delivery.subscribe(() => undefined);
    await waitForRevision(delivery, "1");

    transport.queueRead(state("2"));
    expect(delivery.getSnapshot().snapshot?.revision).toBe("1");
    transport.resume();

    await waitForRevision(delivery, "2");
    expect(transport.readCount()).toBe(2);
    unsubscribe();
  });

  test("coalesces a notice burst into one trailing full read", async () => {
    const transport = controlledAdapter();
    const delivery = createSanitizedDesktopStateDelivery(transport.port);
    const unsubscribe = delivery.subscribe(() => undefined);
    await waitForRevision(delivery, "1");

    const secondRead = deferred<unknown>();
    transport.queueRead(secondRead.promise);
    transport.queueRead(state("4"));
    transport.emit({ revision: "2" });
    await waitFor(() => expect(transport.readCount()).toBe(2));

    transport.emit({ revision: "3" });
    transport.emit({ revision: "4" });
    secondRead.resolve(state("2"));

    await waitForRevision(delivery, "4");
    expect(transport.readCount()).toBe(3);
    unsubscribe();
  });

  test("ends refresh feedback at acknowledgement and advances after commit notice", async () => {
    const transport = controlledAdapter();
    const delivery = createSanitizedDesktopStateDelivery(transport.port);
    const unsubscribe = delivery.subscribe(() => undefined);
    await waitForRevision(delivery, "1");

    const receipt = deferred<unknown>();
    const recoveryRead = deferred<unknown>();
    transport.queueRefresh(receipt.promise);
    transport.queueRead(recoveryRead.promise);
    const firstRefresh = delivery.requestRefresh();
    const joinedRefresh = delivery.requestRefresh();

    expect(joinedRefresh).toBe(firstRefresh);
    expect(delivery.getSnapshot().refreshing).toBe(true);
    receipt.resolve({ accepted: true });
    await firstRefresh;

    expect(transport.refreshCount()).toBe(1);
    expect(delivery.getSnapshot()).toMatchObject({
      phase: "ready",
      refreshing: false,
    });
    expect(delivery.getSnapshot().snapshot?.revision).toBe("1");
    expect(transport.readCount()).toBe(2);

    recoveryRead.resolve(state("1"));
    await Promise.resolve();
    expect(delivery.getSnapshot().snapshot?.revision).toBe("1");

    transport.queueRead(state("2"));
    transport.emit({ revision: "2" });
    await waitForRevision(delivery, "2");
    expect(delivery.getSnapshot().snapshot?.revision).toBe("2");

    transport.queueRefresh({ accepted: "yes" });
    await delivery.requestRefresh();
    expect(delivery.getSnapshot()).toMatchObject({
      phase: "degraded",
      refreshing: false,
    });
    expect(delivery.getSnapshot().snapshot?.revision).toBe("2");
    unsubscribe();
  });

  test("keeps one transport refresh across delivery reactivation", async () => {
    const transport = controlledAdapter();
    const delivery = createSanitizedDesktopStateDelivery(transport.port);
    const stopFirstActivation = delivery.subscribe(() => undefined);
    await waitForRevision(delivery, "1");

    const receipt = deferred<unknown>();
    transport.queueRefresh(receipt.promise);
    const firstRefresh = delivery.requestRefresh();
    stopFirstActivation();

    transport.queueRead(state("1"));
    const stopSecondActivation = delivery.subscribe(() => undefined);
    await waitFor(() => expect(transport.readCount()).toBe(2));
    transport.queueRead(state("2"));

    const joinedRefresh = delivery.requestRefresh();
    expect(joinedRefresh).toBe(firstRefresh);
    expect(delivery.getSnapshot().refreshing).toBe(true);

    receipt.resolve({ accepted: true });
    await joinedRefresh;
    await waitForRevision(delivery, "2");
    expect(transport.refreshCount()).toBe(1);
    expect(delivery.getSnapshot().refreshing).toBe(false);
    stopSecondActivation();
  });

  test("expires a stalled refresh acknowledgement so a later request can retry", async () => {
    vi.useFakeTimers();
    try {
      const stalledReceipt = deferred<unknown>();
      const transport = controlledAdapter();
      const delivery = createSanitizedDesktopStateDelivery(transport.port);
      const unsubscribe = delivery.subscribe(() => undefined);
      await waitForRevision(delivery, "1");

      transport.queueRefresh(stalledReceipt.promise);
      const stalledRefresh = delivery.requestRefresh();
      await Promise.resolve();
      expect(delivery.getSnapshot().refreshing).toBe(true);

      await vi.advanceTimersByTimeAsync(2_000);
      await stalledRefresh;

      expect(delivery.getSnapshot()).toMatchObject({
        phase: "degraded",
        refreshing: false,
        snapshot: { revision: "1" },
      });

      transport.queueRefresh({ accepted: true });
      transport.queueRead(state("2"));
      await delivery.requestRefresh();
      await waitForRevision(delivery, "2");

      expect(transport.refreshCount()).toBe(2);
      stalledReceipt.resolve({ accepted: true });
      unsubscribe();
    } finally {
      vi.useRealTimers();
    }
  });

  test("starts a fresh activation when an abandoned read never settles", async () => {
    const abandonedRead = deferred<unknown>();
    const transport = controlledAdapter(abandonedRead.promise);
    const delivery = createSanitizedDesktopStateDelivery(transport.port);
    const stopFirstActivation = delivery.subscribe(() => undefined);
    await waitFor(() => expect(transport.readCount()).toBe(1));

    stopFirstActivation();
    transport.queueRead(state("2"));
    const stopSecondActivation = delivery.subscribe(() => undefined);

    await waitForRevision(delivery, "2");
    expect(transport.readCount()).toBe(2);

    abandonedRead.resolve(state("3"));
    await abandonedRead.promise;
    await Promise.resolve();
    expect(delivery.getSnapshot().snapshot?.revision).toBe("2");
    stopSecondActivation();
  });

  test("connects a late subscription while the fallback read is still stalled", async () => {
    vi.useFakeTimers();
    try {
      const fallbackRead = deferred<unknown>();
      const connection = deferred<
        SanitizedDesktopStatePortOutcome<
          () => void,
          "invalidation-stream-unavailable"
        >
      >();
      const stopListening = vi.fn();
      const delivery = createSanitizedDesktopStateDelivery({
        readSnapshot: () => fallbackRead.promise.then(success),
        requestRefresh: () => Promise.resolve(success({ accepted: true })),
        subscribeToInvalidations: () => connection.promise,
      });
      const unsubscribe = delivery.subscribe(() => undefined);

      await vi.advanceTimersByTimeAsync(1_000);
      expect(delivery.getSnapshot().phase).toBe("degraded");

      connection.resolve(success(stopListening));
      await Promise.resolve();
      await Promise.resolve();
      unsubscribe();

      expect(stopListening).toHaveBeenCalledOnce();
      fallbackRead.resolve(state("1"));
    } finally {
      vi.useRealTimers();
    }
  });

  test("cleans up a late subscription from a timed-out stale activation", async () => {
    vi.useFakeTimers();
    try {
      const connection = deferred<
        SanitizedDesktopStatePortOutcome<
          () => void,
          "invalidation-stream-unavailable"
        >
      >();
      const stopListening = vi.fn();
      const delivery = createSanitizedDesktopStateDelivery({
        readSnapshot: () => Promise.resolve(success(state("1"))),
        requestRefresh: () => Promise.resolve(success({ accepted: true })),
        subscribeToInvalidations: () => connection.promise,
      });
      const unsubscribe = delivery.subscribe(() => undefined);
      unsubscribe();

      await vi.advanceTimersByTimeAsync(1_000);
      connection.resolve(success(stopListening));
      await Promise.resolve();
      await Promise.resolve();

      expect(stopListening).toHaveBeenCalledOnce();
    } finally {
      vi.useRealTimers();
    }
  });

  test("ignores late reads after teardown", async () => {
    const initialRead = deferred<unknown>();
    const transport = controlledAdapter(initialRead.promise);
    const delivery = createSanitizedDesktopStateDelivery(transport.port);
    const unsubscribe = delivery.subscribe(() => undefined);
    await waitFor(() => expect(transport.readCount()).toBe(1));

    unsubscribe();
    initialRead.resolve(state("2"));
    await initialRead.promise;
    await Promise.resolve();

    expect(delivery.getSnapshot()).toEqual({
      phase: "loading",
      refreshing: false,
      snapshot: null,
    });
    expect(transport.unsubscribeCount()).toBe(1);
  });

  test("keeps the cached snapshot but downgrades it after teardown", async () => {
    const transport = controlledAdapter();
    const delivery = createSanitizedDesktopStateDelivery(transport.port);
    const unsubscribe = delivery.subscribe(() => undefined);
    await waitForRevision(delivery, "1");

    unsubscribe();

    expect(delivery.getSnapshot()).toMatchObject({
      phase: "degraded",
      refreshing: false,
      snapshot: { revision: "1" },
    });
  });

  test("retains a validated snapshot when the notice subscription returns a fault", async () => {
    const operations: string[] = [];
    const delivery = createSanitizedDesktopStateDelivery({
      readSnapshot: () => {
        operations.push("read");
        return Promise.resolve(success(state("1")));
      },
      requestRefresh: () => Promise.resolve(success({ accepted: true })),
      subscribeToInvalidations: () => {
        operations.push("subscribe");
        return Promise.resolve(fault("invalidation-stream-unavailable"));
      },
    });
    const unsubscribe = delivery.subscribe(() => undefined);

    await waitForRevision(delivery, "1");
    expect(operations).toEqual(["subscribe", "read"]);
    expect(delivery.getSnapshot()).toMatchObject({
      phase: "degraded",
      snapshot: { revision: "1" },
    });
    unsubscribe();
  });

  test("falls back to a cached snapshot while notice subscription is stalled", async () => {
    vi.useFakeTimers();
    try {
      const connection = deferred<
        SanitizedDesktopStatePortOutcome<
          () => void,
          "invalidation-stream-unavailable"
        >
      >();
      const reads = [state("1"), state("2")];
      const delivery = createSanitizedDesktopStateDelivery({
        readSnapshot: () => Promise.resolve(success(reads.shift())),
        requestRefresh: () => Promise.resolve(success({ accepted: true })),
        subscribeToInvalidations: () => connection.promise,
      });
      const unsubscribe = delivery.subscribe(() => undefined);

      await vi.advanceTimersByTimeAsync(1_000);
      await waitForRevision(delivery, "1");
      expect(delivery.getSnapshot().phase).toBe("degraded");

      connection.resolve(success(() => undefined));
      await waitForRevision(delivery, "2");
      expect(delivery.getSnapshot().phase).toBe("ready");
      unsubscribe();
    } finally {
      vi.useRealTimers();
    }
  });

});
