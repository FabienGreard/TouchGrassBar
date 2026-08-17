import { REVISION_NOTICE_EVENT } from "@touchgrass/contracts";
import { describe, expect, test, vi } from "vitest";

import {
  createDoomerboardDelivery,
  type DoomerboardPort,
} from "@/native-state/doomerboard-delivery";
import {
  createTauriDoomerboardAdapter,
  type TauriDoomerboardBindings,
} from "@/native-state/tauri-doomerboard-adapter";

const readyView = {
  contractVersion: 1,
  rows: [
    {
      apiEquivalentCostUsd: 12.5,
      displayName: "Fabien",
      rank: 1,
      tokenScore: 4_200_000,
      touchGrassId: "TG-234567",
    },
  ],
  status: "ready",
} as const;

function port(): DoomerboardPort & { changed: () => void } {
  let receiveChange: (() => void) | undefined;
  return {
    changed: () => receiveChange?.(),
    read: vi.fn(async () => ({ ok: true as const, value: readyView })),
    subscribe: vi.fn(async (receive) => {
      receiveChange = receive;
      return { ok: true as const, value: () => undefined };
    }),
  };
}

describe("Doomerboard delivery", () => {
  test("subscribes before its first strict public read and refreshes on notice", async () => {
    const native = port();
    const delivery = createDoomerboardDelivery(native);

    await delivery.activate();
    expect(native.subscribe).toHaveBeenCalledOnce();
    expect(native.read).toHaveBeenCalledOnce();
    expect(delivery.getSnapshot()).toEqual({ phase: "ready", view: readyView });

    native.changed();
    await vi.waitFor(() => expect(native.read).toHaveBeenCalledTimes(2));
  });

  test("rejects private fields and contains native failures", async () => {
    const native = port();
    native.read = vi.fn(async () => ({
      ok: true as const,
      value: {
        ...readyView,
        rows: [{ ...readyView.rows[0], providerMessageId: "private" }],
      },
    }));
    const delivery = createDoomerboardDelivery(native);

    await delivery.activate();
    expect(delivery.getSnapshot()).toEqual({ phase: "degraded", view: null });

    native.read = vi.fn(async () => ({
      fault: { code: "doomerboard-unavailable" as const },
      ok: false as const,
    }));
    await delivery.read();
    expect(delivery.getSnapshot()).toEqual({ phase: "degraded", view: null });
  });
});

describe("Tauri Doomerboard adapter", () => {
  test("uses only the narrow command, revision event, and panel focus", async () => {
    const stopRevision = vi.fn();
    const stopFocus = vi.fn();
    let revision!: () => void;
    let focus!: (event: { payload: boolean }) => void;
    const bindings: TauriDoomerboardBindings = {
      invoke: vi.fn(async () => readyView),
      listen: vi.fn(async (event, receive) => {
        expect(event).toBe(REVISION_NOTICE_EVENT);
        revision = () => receive({ payload: {} });
        return stopRevision;
      }),
      onFocusChanged: vi.fn(async (receive) => {
        focus = receive;
        return stopFocus;
      }),
    };
    const receive = vi.fn();
    const adapter = createTauriDoomerboardAdapter(bindings);

    expect(await adapter.read()).toEqual({ ok: true, value: readyView });
    const subscription = await adapter.subscribe(receive);
    expect(bindings.invoke).toHaveBeenCalledWith("get_global_doomerboard");
    expect(subscription.ok).toBe(true);
    revision();
    focus({ payload: false });
    focus({ payload: true });
    expect(receive).toHaveBeenCalledTimes(2);

    if (subscription.ok) subscription.value();
    expect(stopRevision).toHaveBeenCalledOnce();
    expect(stopFocus).toHaveBeenCalledOnce();
  });
});
