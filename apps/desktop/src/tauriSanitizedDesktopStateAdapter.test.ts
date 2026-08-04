import { REVISION_NOTICE_EVENT } from "@touchgrass/contracts";
import { describe, expect, test, vi } from "vitest";

import {
  createTauriSanitizedDesktopStateAdapter,
  type TauriSanitizedDesktopStateBindings,
} from "@/tauriSanitizedDesktopStateAdapter";

function deferred<Value>() {
  let resolve!: (value: Value) => void;
  const promise = new Promise<Value>((nextResolve) => {
    resolve = nextResolve;
  });
  return { promise, resolve };
}

describe("Tauri Sanitized Desktop State adapter", () => {
  test("maps native revision and focus invalidations and cleans up both listeners", async () => {
    const stopRevisionNotices = vi.fn();
    const stopFocusChanges = vi.fn();
    let emitRevision!: (event: { payload: unknown }) => void;
    let emitFocusChange!: (event: { payload: boolean }) => void;

    const bindings: TauriSanitizedDesktopStateBindings = {
      invoke: vi.fn(async () => undefined),
      listen: vi.fn(async (event, receive) => {
        expect(event).toBe(REVISION_NOTICE_EVENT);
        emitRevision = receive;
        return stopRevisionNotices;
      }),
      onFocusChanged: vi.fn(async (receive) => {
        emitFocusChange = receive;
        return stopFocusChanges;
      }),
    };
    const receiveInvalidation = vi.fn();
    const adapter = createTauriSanitizedDesktopStateAdapter(bindings);

    const subscription = await adapter.subscribeToInvalidations(
      receiveInvalidation,
    );
    expect(subscription.ok).toBe(true);
    if (!subscription.ok) throw new Error("expected connected adapter");

    emitRevision({ payload: { revision: "7" } });
    emitFocusChange({ payload: false });
    emitFocusChange({ payload: true });

    expect(receiveInvalidation.mock.calls).toEqual([
      [{ kind: "revision", notice: { revision: "7" } }],
      [{ kind: "surface-resumed" }],
    ]);

    subscription.value();
    expect(stopRevisionNotices).toHaveBeenCalledOnce();
    expect(stopFocusChanges).toHaveBeenCalledOnce();
  });

  test("contains raw invoke and listener failures behind closed faults", async () => {
    const privateFailure = new Error("private Tauri transport detail");
    const bindings: TauriSanitizedDesktopStateBindings = {
      invoke: vi.fn(() => Promise.reject(privateFailure)),
      listen: vi.fn(() => Promise.reject(privateFailure)),
      onFocusChanged: vi.fn(() => Promise.reject(privateFailure)),
    };
    const adapter = createTauriSanitizedDesktopStateAdapter(bindings);

    expect(await adapter.readSnapshot()).toEqual({
      fault: { code: "snapshot-unavailable" },
      ok: false,
    });
    expect(await adapter.requestRefresh()).toEqual({
      fault: { code: "refresh-unavailable" },
      ok: false,
    });
    expect(
      await adapter.subscribeToInvalidations(() => undefined),
    ).toEqual({
      fault: { code: "invalidation-stream-unavailable" },
      ok: false,
    });
    expect(bindings.onFocusChanged).not.toHaveBeenCalled();
  });

  test("contains focus-listener setup failures and cleans the revision listener", async () => {
    const stopRevisionNotices = vi.fn();
    const bindings: TauriSanitizedDesktopStateBindings = {
      invoke: vi.fn(async () => undefined),
      listen: vi.fn(async () => stopRevisionNotices),
      onFocusChanged: vi.fn(() =>
        Promise.reject(new Error("private focus-listener detail")),
      ),
    };
    const adapter = createTauriSanitizedDesktopStateAdapter(bindings);

    expect(
      await adapter.subscribeToInvalidations(() => undefined),
    ).toEqual({
      fault: { code: "invalidation-stream-unavailable" },
      ok: false,
    });
    expect(stopRevisionNotices).toHaveBeenCalledOnce();
  });

  test("times out partial listener setup and cleans late resources", async () => {
    vi.useFakeTimers();
    try {
      const stopRevisionNotices = vi.fn();
      const stopFocusChanges = vi.fn();
      const focusListener = deferred<() => void>();
      const bindings: TauriSanitizedDesktopStateBindings = {
        invoke: vi.fn(async () => undefined),
        listen: vi.fn(async () => stopRevisionNotices),
        onFocusChanged: vi.fn(() => focusListener.promise),
      };
      const adapter = createTauriSanitizedDesktopStateAdapter(bindings);

      const subscription = adapter.subscribeToInvalidations(() => undefined);
      await vi.advanceTimersByTimeAsync(1_000);

      expect(await subscription).toEqual({
        fault: { code: "invalidation-stream-unavailable" },
        ok: false,
      });
      expect(stopRevisionNotices).toHaveBeenCalledOnce();

      focusListener.resolve(stopFocusChanges);
      await Promise.resolve();
      await Promise.resolve();
      expect(stopFocusChanges).toHaveBeenCalledOnce();
    } finally {
      vi.useRealTimers();
    }
  });
});
