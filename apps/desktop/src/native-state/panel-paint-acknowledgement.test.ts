import { PANEL_PAINT_REQUEST_EVENT } from "@touchgrass/contracts";
import { describe, expect, test, vi } from "vitest";

import {
  activatePanelPaintAcknowledgement,
  type PanelPaintBindings,
} from "./panel-paint-acknowledgement";

function frameHarness() {
  let nextId = 1;
  const callbacks = new Map<number, FrameRequestCallback>();
  return {
    cancelAnimationFrame: vi.fn((id: number) => callbacks.delete(id)),
    flush() {
      const frame = callbacks.entries().next().value as
        | [number, FrameRequestCallback]
        | undefined;
      if (!frame) throw new Error("No animation frame is pending.");
      callbacks.delete(frame[0]);
      frame[1](performance.now());
    },
    pending: () => callbacks.size,
    requestAnimationFrame: vi.fn((callback: FrameRequestCallback) => {
      const id = nextId++;
      callbacks.set(id, callback);
      return id;
    }),
  };
}

describe("panel paint acknowledgement", () => {
  test("acknowledges only after one complete visible frame", async () => {
    let receive!: (payload: { payload: { sequence: number } }) => void;
    const frames = frameHarness();
    const stopListening = vi.fn();
    const bindings: PanelPaintBindings = {
      acknowledge: vi.fn(async () => undefined),
      cancelAnimationFrame: frames.cancelAnimationFrame,
      listen: vi.fn(async (event, onEvent) => {
        expect(event).toBe(PANEL_PAINT_REQUEST_EVENT);
        receive = onEvent;
        return stopListening;
      }),
      prepare: vi.fn(async () => undefined),
      requestAnimationFrame: frames.requestAnimationFrame,
      takeRequest: vi.fn(async () => null),
    };

    const stop = await activatePanelPaintAcknowledgement(bindings);
    receive({ payload: { sequence: 7 } });

    expect(bindings.acknowledge).not.toHaveBeenCalled();
    await vi.waitFor(() => expect(frames.pending()).toBe(1));
    frames.flush();
    expect(bindings.acknowledge).not.toHaveBeenCalled();
    frames.flush();
    await vi.waitFor(() =>
      expect(bindings.acknowledge).toHaveBeenCalledWith(7),
    );

    stop();
    expect(stopListening).toHaveBeenCalledOnce();
  });

  test("uses the pending-request fallback and replaces a stale frame", async () => {
    let receive!: (payload: { payload: { sequence: number } }) => void;
    const frames = frameHarness();
    const acknowledge = vi.fn(async () => undefined);
    const bindings: PanelPaintBindings = {
      acknowledge,
      cancelAnimationFrame: frames.cancelAnimationFrame,
      listen: vi.fn(async (_event, onEvent) => {
        receive = onEvent;
        return () => undefined;
      }),
      prepare: vi.fn(async () => undefined),
      requestAnimationFrame: frames.requestAnimationFrame,
      takeRequest: vi.fn(async () => ({ sequence: 8 })),
    };

    const stop = await activatePanelPaintAcknowledgement(bindings);
    await vi.waitFor(() => expect(frames.pending()).toBe(1));

    receive({ payload: { sequence: 9 } });
    expect(frames.cancelAnimationFrame).toHaveBeenCalled();
    await vi.waitFor(() => expect(frames.pending()).toBe(1));
    frames.flush();
    frames.flush();
    await vi.waitFor(() => expect(acknowledge).toHaveBeenCalledWith(9));
    expect(acknowledge).not.toHaveBeenCalledWith(8);

    stop();
  });

  test("waits for fonts, images, and the final native resize", async () => {
    let receive!: (payload: { payload: { sequence: number } }) => void;
    let finishPreparing!: () => void;
    const frames = frameHarness();
    const prepare = vi.fn(
      () =>
        new Promise<void>((resolve) => {
          finishPreparing = resolve;
        }),
    );
    const bindings: PanelPaintBindings = {
      acknowledge: vi.fn(async () => undefined),
      cancelAnimationFrame: frames.cancelAnimationFrame,
      listen: vi.fn(async (_event, onEvent) => {
        receive = onEvent;
        return () => undefined;
      }),
      prepare,
      requestAnimationFrame: frames.requestAnimationFrame,
      takeRequest: vi.fn(async () => null),
    };

    const stop = await activatePanelPaintAcknowledgement(bindings);
    receive({ payload: { sequence: 11 } });
    expect(prepare).toHaveBeenCalledOnce();
    expect(frames.pending()).toBe(0);

    finishPreparing();
    await vi.waitFor(() => expect(frames.pending()).toBe(1));
    frames.flush();
    frames.flush();
    expect(bindings.acknowledge).not.toHaveBeenCalled();
    expect(prepare).toHaveBeenCalledTimes(2);

    finishPreparing();
    await vi.waitFor(() =>
      expect(bindings.acknowledge).toHaveBeenCalledWith(11),
    );
    stop();
  });
});
