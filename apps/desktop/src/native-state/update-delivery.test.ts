import { describe, expect, test, vi } from "vitest";

import {
  createTauriUpdateAdapter,
  type TauriUpdateBindings,
} from "@/native-state/tauri-update-adapter";
import { createUpdateDelivery, type UpdatePort } from "@/native-state/update-delivery";

const idleState = {
  automaticChecksEnabled: true,
  contractVersion: 2,
  currentVersion: "1.3.2",
  onlineFeaturesPaused: false,
  update: { status: "idle" },
} as const;

const availableState = {
  ...idleState,
  update: { status: "available", version: "1.4.0" },
} as const;

function port(): UpdatePort & { changed: () => void } {
  let receiveChange: (() => void) | undefined;
  return {
    changed: () => receiveChange?.(),
    check: vi.fn(async () => ({
      ok: true as const,
      value: { ...idleState, update: { status: "checking" } },
    })),
    install: vi.fn(async () => ({ ok: true as const, value: idleState })),
    openLatestDmg: vi.fn(async () => ({
      ok: true as const,
      value: undefined,
    })),
    openSource: vi.fn(async () => ({
      ok: true as const,
      value: undefined,
    })),
    read: vi.fn(async () => ({ ok: true as const, value: idleState })),
    retry: vi.fn(async () => ({ ok: true as const, value: idleState })),
    setAutomaticChecks: vi.fn(async (enabled) => ({
      ok: true as const,
      value: { ...idleState, automaticChecksEnabled: enabled },
    })),
    subscribe: vi.fn(async (receive) => {
      receiveChange = receive;
      return { ok: true as const, value: () => undefined };
    }),
  };
}

describe("update delivery", () => {
  test("subscribes before its first bounded state read and refreshes on notice", async () => {
    const native = port();
    const delivery = createUpdateDelivery(native);
    await delivery.activate();

    expect(native.subscribe).toHaveBeenCalledOnce();
    expect(native.read).toHaveBeenCalledOnce();
    expect(delivery.getSnapshot()).toEqual({ phase: "ready", state: idleState });
    native.changed();
    await vi.waitFor(() => expect(native.read).toHaveBeenCalledTimes(2));
  });

  test("queues a follow-up read when a notice arrives during a pending read", async () => {
    const native = port();
    let completeFirstRead: ((outcome: { ok: true; value: unknown }) => void) | undefined;
    const firstRead = new Promise<{ ok: true; value: unknown }>((resolve) => {
      completeFirstRead = resolve;
    });
    native.read = vi
      .fn()
      .mockReturnValueOnce(firstRead)
      .mockResolvedValueOnce({ ok: true, value: availableState });
    const delivery = createUpdateDelivery(native);

    const activation = delivery.activate();
    await vi.waitFor(() => expect(native.read).toHaveBeenCalledOnce());
    native.changed();
    if (!completeFirstRead) throw new Error("first update read did not start");
    completeFirstRead({ ok: true, value: idleState });
    await activation;

    expect(native.read).toHaveBeenCalledTimes(2);
    expect(delivery.getSnapshot()).toEqual({
      phase: "ready",
      state: availableState,
    });
  });

  test("rejects unknown contract fields and contains port failures", async () => {
    const native = port();
    native.read = vi.fn(async () => ({
      ok: true as const,
      value: { ...idleState, privatePath: "/private" },
    }));
    native.check = vi.fn(async () => ({
      fault: { code: "update-check-unavailable" as const },
      ok: false as const,
    }));
    const delivery = createUpdateDelivery(native);

    await delivery.activate();
    expect(delivery.getSnapshot()).toEqual({ phase: "degraded", state: null });
    expect(await delivery.check()).toBe(false);
    expect(delivery.getSnapshot().phase).toBe("degraded");
  });

  test("routes only explicit user update actions", async () => {
    const native = port();
    const delivery = createUpdateDelivery(native);
    await delivery.activate();

    expect(await delivery.check()).toBe(true);
    expect(await delivery.install()).toBe(true);
    expect(await delivery.retry()).toBe(true);
    expect(await delivery.openLatestDmg()).toBe(true);
    expect(await delivery.openSource()).toBe(true);
    expect(await delivery.setAutomaticChecks(false)).toBe(true);
    expect(native.check).toHaveBeenCalledOnce();
    expect(native.install).toHaveBeenCalledOnce();
    expect(native.retry).toHaveBeenCalledOnce();
    expect(native.openLatestDmg).toHaveBeenCalledOnce();
    expect(native.openSource).toHaveBeenCalledOnce();
    expect(native.setAutomaticChecks).toHaveBeenCalledWith(false);
    expect(delivery.getSnapshot().state?.automaticChecksEnabled).toBe(false);
  });
});

describe("Tauri update adapter", () => {
  test("uses the narrow update commands and invalidation event", async () => {
    const stop = vi.fn();
    const bindings: TauriUpdateBindings = {
      invoke: vi.fn(async (command) => ({ command })),
      listen: vi.fn(async () => stop),
    };
    const adapter = createTauriUpdateAdapter(bindings);
    const receive = vi.fn();

    await adapter.read();
    await adapter.check();
    await adapter.install();
    await adapter.retry();
    await adapter.openLatestDmg();
    await adapter.openSource();
    await adapter.setAutomaticChecks(false);
    const subscription = await adapter.subscribe(receive);

    expect(bindings.invoke).toHaveBeenNthCalledWith(1, "get_update_state");
    expect(bindings.invoke).toHaveBeenNthCalledWith(2, "check_for_updates");
    expect(bindings.invoke).toHaveBeenNthCalledWith(3, "install_update");
    expect(bindings.invoke).toHaveBeenNthCalledWith(4, "retry_update");
    expect(bindings.invoke).toHaveBeenNthCalledWith(5, "open_latest_dmg");
    expect(bindings.invoke).toHaveBeenNthCalledWith(6, "open_source_repository");
    expect(bindings.invoke).toHaveBeenNthCalledWith(7, "set_automatic_update_checks", {
      enabled: false,
    });
    expect(bindings.listen).toHaveBeenCalledWith("update-state-changed", receive);
    if (subscription.ok) subscription.value();
    expect(stop).toHaveBeenCalledOnce();
  });

  test("does not expose native failures", async () => {
    const bindings: TauriUpdateBindings = {
      invoke: vi.fn(() => Promise.reject(new Error("private path"))),
      listen: vi.fn(() => Promise.reject(new Error("private path"))),
    };
    const adapter = createTauriUpdateAdapter(bindings);

    expect(await adapter.read()).toEqual({
      fault: { code: "update-state-unavailable" },
      ok: false,
    });
    expect(await adapter.install()).toEqual({
      fault: { code: "update-install-unavailable" },
      ok: false,
    });
    expect(await adapter.subscribe(() => undefined)).toEqual({
      fault: { code: "update-state-stream-unavailable" },
      ok: false,
    });
  });
});
