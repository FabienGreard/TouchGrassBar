import { describe, expect, test, vi } from "vitest";

import {
  createTauriUpdateAdapter,
  type TauriUpdateBindings,
} from "@/native-state/tauri-update-adapter";
import {
  createUpdateDelivery,
  type UpdatePort,
} from "@/native-state/update-delivery";

const idleState = {
  contractVersion: 1,
  currentVersion: "1.3.2",
  onlineFeaturesPaused: false,
  update: { status: "idle" },
} as const;

function port(): UpdatePort & { changed: () => void } {
  let changed = () => undefined;
  return {
    changed: () => changed(),
    check: vi.fn(async () => ({
      ok: true as const,
      value: { ...idleState, update: { status: "checking" } },
    })),
    defer: vi.fn(async () => ({ ok: true as const, value: idleState })),
    install: vi.fn(async () => ({ ok: true as const, value: idleState })),
    openLatestDmg: vi.fn(async () => ({
      ok: true as const,
      value: undefined,
    })),
    read: vi.fn(async () => ({ ok: true as const, value: idleState })),
    retry: vi.fn(async () => ({ ok: true as const, value: idleState })),
    subscribe: vi.fn(async (receive) => {
      changed = receive;
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
    expect(await delivery.defer()).toBe(true);
    expect(await delivery.install()).toBe(true);
    expect(await delivery.retry()).toBe(true);
    expect(await delivery.openLatestDmg()).toBe(true);
    expect(native.check).toHaveBeenCalledOnce();
    expect(native.defer).toHaveBeenCalledOnce();
    expect(native.install).toHaveBeenCalledOnce();
    expect(native.retry).toHaveBeenCalledOnce();
    expect(native.openLatestDmg).toHaveBeenCalledOnce();
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
    await adapter.defer();
    await adapter.install();
    await adapter.retry();
    await adapter.openLatestDmg();
    const subscription = await adapter.subscribe(receive);

    expect(bindings.invoke).toHaveBeenNthCalledWith(1, "get_update_state");
    expect(bindings.invoke).toHaveBeenNthCalledWith(2, "check_for_updates");
    expect(bindings.invoke).toHaveBeenNthCalledWith(3, "defer_update");
    expect(bindings.invoke).toHaveBeenNthCalledWith(4, "install_update");
    expect(bindings.invoke).toHaveBeenNthCalledWith(5, "retry_update");
    expect(bindings.invoke).toHaveBeenNthCalledWith(6, "open_latest_dmg");
    expect(bindings.listen).toHaveBeenCalledWith(
      "update-state-changed",
      receive,
    );
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
