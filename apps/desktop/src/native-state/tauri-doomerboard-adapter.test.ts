import { REVISION_NOTICE_EVENT } from "@touchgrass/contracts";
import { describe, expect, test, vi } from "vitest";

import { defaultDoomerboardQuery } from "@/native-state/doomerboard-query";
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

describe("Tauri Doomerboard adapter", () => {
  test("reports panel focus without reading a Doomerboard", async () => {
    const stopFocus = vi.fn();
    let focus!: (event: { payload: boolean }) => void;
    const bindings: TauriDoomerboardBindings = {
      invoke: vi.fn(async () => readyView),
      listen: vi.fn(async () => vi.fn()),
      onFocusChanged: vi.fn(async (receive) => {
        focus = receive;
        return stopFocus;
      }),
    };
    const adapter = createTauriDoomerboardAdapter(bindings);
    const focused = vi.fn();

    const subscription = await adapter.subscribeFocus(focused);
    if (!subscription.ok) throw new Error("expected focus subscription");
    focus({ payload: false });
    focus({ payload: true });

    expect(focused.mock.calls).toEqual([[false], [true]]);
    expect(bindings.invoke).not.toHaveBeenCalled();
    subscription.value();
    expect(stopFocus).toHaveBeenCalledOnce();
  });

  test("uses the narrow Add Tokenmaxxer command", async () => {
    const outcome = { contractVersion: 1, status: "added" } as const;
    const bindings: TauriDoomerboardBindings = {
      invoke: vi.fn(async () => outcome),
      listen: vi.fn(async () => vi.fn()),
      onFocusChanged: vi.fn(async () => vi.fn()),
    };
    const adapter = createTauriDoomerboardAdapter(bindings);

    await expect(adapter.add("TG-7K4P9D", "TG-234567")).resolves.toEqual({
      ok: true,
      value: outcome,
    });
    expect(bindings.invoke).toHaveBeenCalledWith("add_tokenmaxxer", {
      profileKey: "TG-7K4P9D",
      touchGrassId: "TG-234567",
    });
  });

  test("uses only the narrow read command and revision event for data changes", async () => {
    const stopRevision = vi.fn();
    let revision!: () => void;
    const bindings: TauriDoomerboardBindings = {
      invoke: vi.fn(async () => readyView),
      listen: vi.fn(async (event, receive) => {
        expect(event).toBe(REVISION_NOTICE_EVENT);
        revision = () => receive({ payload: {} });
        return stopRevision;
      }),
      onFocusChanged: vi.fn(async () => vi.fn()),
    };
    const receive = vi.fn();
    const adapter = createTauriDoomerboardAdapter(bindings);

    await expect(adapter.read("TG-7K4P9D", defaultDoomerboardQuery)).resolves.toEqual({
      ok: true,
      value: readyView,
    });
    const subscription = await adapter.subscribe(receive);
    expect(bindings.invoke).toHaveBeenCalledWith("get_doomerboard", {
      profileKey: "TG-7K4P9D",
      query: defaultDoomerboardQuery,
      requestId: expect.any(String),
    });
    if (!subscription.ok) throw new Error("expected data subscription");
    revision();

    expect(receive).toHaveBeenCalledOnce();
    expect(bindings.onFocusChanged).not.toHaveBeenCalled();
    subscription.value();
    expect(stopRevision).toHaveBeenCalledOnce();
  });

  test("cancels the matching native read when its signal aborts", async () => {
    let announceRead!: () => void;
    let finishRead!: () => void;
    const readStarted = new Promise<void>((resolve) => {
      announceRead = resolve;
    });
    const readFinished = new Promise<void>((resolve) => {
      finishRead = resolve;
    });
    let requestId: unknown;
    const bindings: TauriDoomerboardBindings = {
      invoke: vi.fn(async (command, args) => {
        if (command === "get_doomerboard") {
          requestId = args?.requestId;
          announceRead();
          await readFinished;
          return readyView;
        }
        if (command === "cancel_doomerboard_read") {
          finishRead();
          return undefined;
        }
        return undefined;
      }),
      listen: vi.fn(async () => vi.fn()),
      onFocusChanged: vi.fn(async () => vi.fn()),
    };
    const adapter = createTauriDoomerboardAdapter(bindings);
    const controller = new AbortController();
    const read = adapter.read("TG-7K4P9D", defaultDoomerboardQuery, controller.signal);

    await readStarted;
    controller.abort();
    await vi.waitFor(() =>
      expect(bindings.invoke).toHaveBeenCalledWith("cancel_doomerboard_read", { requestId }),
    );
    await expect(read).resolves.toEqual({ ok: true, value: readyView });
  });

  test("refreshes at UTC rollover and cancels the next rollover on cleanup", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-08-17T23:59:59.900Z"));
    try {
      const stopRevision = vi.fn();
      const bindings: TauriDoomerboardBindings = {
        invoke: vi.fn(async () => readyView),
        listen: vi.fn(async () => stopRevision),
        onFocusChanged: vi.fn(async () => vi.fn()),
      };
      const receive = vi.fn();
      const adapter = createTauriDoomerboardAdapter(bindings);
      const subscription = await adapter.subscribe(receive);
      if (!subscription.ok) throw new Error("expected data subscription");

      await vi.advanceTimersByTimeAsync(99);
      expect(receive).not.toHaveBeenCalled();
      await vi.advanceTimersByTimeAsync(1);
      expect(receive).toHaveBeenCalledOnce();

      subscription.value();
      await vi.advanceTimersByTimeAsync(24 * 60 * 60 * 1_000);
      expect(receive).toHaveBeenCalledOnce();
      expect(stopRevision).toHaveBeenCalledOnce();
    } finally {
      vi.useRealTimers();
    }
  });

  test("does not poll remote scores outside the query cache", async () => {
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
      if (!subscription.ok) throw new Error("expected data subscription");

      await vi.advanceTimersByTimeAsync(10 * 60 * 1_000);
      expect(receive).not.toHaveBeenCalled();
      subscription.value();
    } finally {
      vi.useRealTimers();
    }
  });
});
