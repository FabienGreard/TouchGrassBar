import { REVISION_NOTICE_EVENT } from "@touchgrass/contracts";
import { describe, expect, test, vi } from "vitest";

import {
  createDoomerboardDelivery,
  defaultDoomerboardQuery,
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

const selectedView = {
  ...readyView,
  rows: [
    {
      ...readyView.rows[0],
      displayName: "Selected Fabien",
      tokenScore: 8_400_000,
    },
  ],
} as const;

const refreshedView = {
  ...readyView,
  rows: [
    {
      ...readyView.rows[0],
      displayName: "Fresh Fabien",
      tokenScore: 12_600_000,
    },
  ],
} as const;

function port(): DoomerboardPort & { changed: () => void } {
  let receiveChange: (() => void) | undefined;
  return {
    add: vi.fn(async () => ({
      ok: true as const,
      value: { contractVersion: 1, status: "added" },
    })),
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

  test("accepts only strict Add Tokenmaxxer outcomes", async () => {
    const native = port();
    const delivery = createDoomerboardDelivery(native);

    await expect(delivery.addTokenmaxxer("TG-234567")).resolves.toEqual({
      contractVersion: 1,
      status: "added",
    });
    native.add = vi.fn(async () => ({
      ok: true as const,
      value: { contractVersion: 1, session: "private", status: "added" },
    }));
    await expect(delivery.addTokenmaxxer("TG-234567")).resolves.toEqual({
      contractVersion: 1,
      status: "unavailable",
    });
  });

  test("reads each selected audience, provider, and period", async () => {
    const native = port();
    const delivery = createDoomerboardDelivery(native);
    await delivery.activate();

    const query = {
      audience: "mine",
      scope: "claude",
      windowDays: 30,
    } as const;
    await delivery.select(query);

    expect(native.read).toHaveBeenNthCalledWith(1, defaultDoomerboardQuery);
    expect(native.read).toHaveBeenNthCalledWith(2, query);
    expect(delivery.getSnapshot()).toEqual({ phase: "ready", view: readyView });
  });

  test("keeps the current rankings visible while a new selection loads", async () => {
    let finishSelection!: () => void;
    const selectionStarted = new Promise<void>((resolve) => {
      finishSelection = resolve;
    });
    const native = port();
    native.read = vi
      .fn()
      .mockResolvedValueOnce({ ok: true as const, value: readyView })
      .mockImplementationOnce(async () => {
        await selectionStarted;
        return { ok: true as const, value: selectedView };
      });
    const delivery = createDoomerboardDelivery(native);
    await delivery.activate();

    const selecting = delivery.select({
      audience: "global",
      scope: "codex",
      windowDays: 7,
    });

    expect(delivery.getSnapshot()).toEqual({ phase: "ready", view: readyView });
    finishSelection();
    await selecting;
    expect(delivery.getSnapshot()).toEqual({
      phase: "ready",
      view: selectedView,
    });
  });

  test("restores cached Global rankings while their refresh is pending", async () => {
    let finishGlobalRefresh!: () => void;
    const native = port();
    native.read = vi
      .fn()
      .mockResolvedValueOnce({ ok: true as const, value: readyView })
      .mockResolvedValueOnce({ ok: true as const, value: selectedView })
      .mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            finishGlobalRefresh = () => resolve({ ok: true as const, value: refreshedView });
          }),
      );
    const delivery = createDoomerboardDelivery(native);
    await delivery.activate();
    await delivery.select({ ...defaultDoomerboardQuery, audience: "mine" });

    const selectingGlobal = delivery.select(defaultDoomerboardQuery);

    expect(native.read).toHaveBeenCalledTimes(3);
    expect(delivery.getSnapshot()).toEqual({ phase: "ready", view: readyView });
    finishGlobalRefresh();
    await selectingGlobal;
    expect(delivery.getSnapshot()).toEqual({ phase: "ready", view: refreshedView });
  });

  test("does not label old rankings as a new unavailable selection", async () => {
    const native = port();
    native.read = vi
      .fn()
      .mockResolvedValueOnce({ ok: true as const, value: readyView })
      .mockResolvedValueOnce({
        fault: { code: "doomerboard-unavailable" as const },
        ok: false as const,
      });
    const delivery = createDoomerboardDelivery(native);
    await delivery.activate();

    await delivery.select({
      audience: "global",
      scope: "claude",
      windowDays: 30,
    });

    expect(delivery.getSnapshot()).toEqual({
      phase: "degraded",
      view: null,
    });
  });

  test("keeps ready rows when the same query becomes unavailable", async () => {
    const native = port();
    native.read = vi
      .fn()
      .mockResolvedValueOnce({ ok: true as const, value: readyView })
      .mockResolvedValueOnce({
        ok: true as const,
        value: { contractVersion: 1, status: "unavailable" },
      });
    const delivery = createDoomerboardDelivery(native);
    await delivery.activate();

    await delivery.read();

    expect(delivery.getSnapshot()).toEqual({
      phase: "degraded",
      view: readyView,
    });
  });

  test("ignores a failed superseded query while the latest selection loads", async () => {
    let rejectSuperseded!: (reason: Error) => void;
    let finishLatest!: () => void;
    const native = port();
    native.read = vi
      .fn()
      .mockResolvedValueOnce({ ok: true as const, value: readyView })
      .mockImplementationOnce(
        () =>
          new Promise((_resolve, reject) => {
            rejectSuperseded = reject;
          }),
      )
      .mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            finishLatest = () => resolve({ ok: true as const, value: selectedView });
          }),
      );
    const delivery = createDoomerboardDelivery(native);
    await delivery.activate();

    const superseded = delivery.select({
      audience: "global",
      scope: "codex",
      windowDays: 7,
    });
    const latest = delivery.select({
      audience: "global",
      scope: "claude",
      windowDays: 30,
    });
    rejectSuperseded(new Error("superseded read failed"));
    await vi.waitFor(() => expect(native.read).toHaveBeenCalledTimes(3));

    expect(delivery.getSnapshot()).toEqual({ phase: "ready", view: readyView });
    finishLatest();
    await Promise.all([superseded, latest]);
    expect(delivery.getSnapshot()).toEqual({
      phase: "ready",
      view: selectedView,
    });
  });
});

describe("Tauri Doomerboard adapter", () => {
  test("reads on activation and whenever the panel regains focus", async () => {
    let focus!: (event: { payload: boolean }) => void;
    const bindings: TauriDoomerboardBindings = {
      invoke: vi.fn(async () => readyView),
      listen: vi.fn(async () => vi.fn()),
      onFocusChanged: vi.fn(async (receive) => {
        focus = receive;
        return vi.fn();
      }),
    };
    const delivery = createDoomerboardDelivery(createTauriDoomerboardAdapter(bindings));

    const unsubscribe = await delivery.activate();
    expect(bindings.invoke).toHaveBeenCalledOnce();

    focus({ payload: false });
    expect(bindings.invoke).toHaveBeenCalledOnce();
    focus({ payload: true });
    await vi.waitFor(() => expect(bindings.invoke).toHaveBeenCalledTimes(2));

    unsubscribe();
  });

  test("uses the narrow Add Tokenmaxxer command", async () => {
    const outcome = { contractVersion: 1, status: "added" } as const;
    const bindings: TauriDoomerboardBindings = {
      invoke: vi.fn(async () => outcome),
      listen: vi.fn(async () => vi.fn()),
      onFocusChanged: vi.fn(async () => vi.fn()),
    };
    const adapter = createTauriDoomerboardAdapter(bindings);

    await expect(adapter.add("TG-234567")).resolves.toEqual({ ok: true, value: outcome });
    expect(bindings.invoke).toHaveBeenCalledWith("add_tokenmaxxer", {
      touchGrassId: "TG-234567",
    });
  });

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

    expect(await adapter.read(defaultDoomerboardQuery)).toEqual({
      ok: true,
      value: readyView,
    });
    const subscription = await adapter.subscribe(receive);
    expect(bindings.invoke).toHaveBeenCalledWith("get_doomerboard", {
      query: {
        audience: "global",
        scope: "combined",
        windowDays: 1,
      },
    });
    expect(subscription.ok).toBe(true);
    revision();
    focus({ payload: false });
    focus({ payload: true });
    expect(receive).toHaveBeenCalledTimes(2);

    if (subscription.ok) subscription.value();
    expect(stopRevision).toHaveBeenCalledOnce();
    expect(stopFocus).toHaveBeenCalledOnce();
  });

  test("refreshes at UTC rollover and cancels the next rollover on cleanup", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-08-17T23:59:59.900Z"));
    try {
      const stopRevision = vi.fn();
      const stopFocus = vi.fn();
      const bindings: TauriDoomerboardBindings = {
        invoke: vi.fn(async () => readyView),
        listen: vi.fn(async () => stopRevision),
        onFocusChanged: vi.fn(async () => stopFocus),
      };
      const receive = vi.fn();
      const adapter = createTauriDoomerboardAdapter(bindings);
      const subscription = await adapter.subscribe(receive);
      if (!subscription.ok) throw new Error("expected connected adapter");

      await vi.advanceTimersByTimeAsync(99);
      expect(receive).not.toHaveBeenCalled();
      await vi.advanceTimersByTimeAsync(1);
      expect(receive).toHaveBeenCalledOnce();

      subscription.value();
      await vi.advanceTimersByTimeAsync(24 * 60 * 60 * 1_000);
      expect(receive).toHaveBeenCalledOnce();
      expect(stopRevision).toHaveBeenCalledOnce();
      expect(stopFocus).toHaveBeenCalledOnce();
    } finally {
      vi.useRealTimers();
    }
  });

  test("polls remote scores and cancels polling on cleanup", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-08-17T12:00:00.000Z"));
    try {
      const bindings: TauriDoomerboardBindings = {
        invoke: vi.fn(async () => readyView),
        listen: vi.fn(async () => vi.fn()),
        onFocusChanged: vi.fn(async () => vi.fn()),
      };
      const receive = vi.fn();
      const adapter = createTauriDoomerboardAdapter(bindings);
      const subscription = await adapter.subscribe(receive);
      if (!subscription.ok) throw new Error("expected connected adapter");

      await vi.advanceTimersByTimeAsync(5 * 60 * 1_000 - 1);
      expect(receive).not.toHaveBeenCalled();
      await vi.advanceTimersByTimeAsync(1);
      expect(receive).toHaveBeenCalledOnce();

      subscription.value();
      await vi.advanceTimersByTimeAsync(5 * 60 * 1_000);
      expect(receive).toHaveBeenCalledOnce();
    } finally {
      vi.useRealTimers();
    }
  });
});
