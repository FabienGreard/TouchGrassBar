import { PANEL_ADD_TOKENMAXXER_EVENT } from "@touchgrass/contracts";
import { afterEach, describe, expect, test, vi } from "vitest";

import { subscribeToPanelAddTokenmaxxer } from "./panel-add-tokenmaxxer";

afterEach(() => vi.useRealTimers());

describe("panel Add Tokenmaxxer requests", () => {
  test("delivers pending requests through the event and registration fallback", async () => {
    let pending = false;
    let notify!: () => void;
    const takeRequest = vi.fn(async () => {
      const requested = pending;
      pending = false;
      return requested;
    });
    const receive = vi.fn();
    const stop = vi.fn();
    const listen = vi.fn(async (event: string, onEvent: () => void) => {
      expect(event).toBe(PANEL_ADD_TOKENMAXXER_EVENT);
      notify = onEvent;
      return stop;
    });

    pending = true;
    const unsubscribe = await subscribeToPanelAddTokenmaxxer(receive, listen, takeRequest);
    expect(receive).toHaveBeenCalledOnce();

    pending = true;
    notify();
    await vi.waitFor(() => expect(receive).toHaveBeenCalledTimes(2));
    unsubscribe();
    expect(stop).toHaveBeenCalledOnce();

    vi.useFakeTimers();
    const failedListen = vi.fn(async () => {
      throw new Error("listener unavailable");
    });
    const stopFallback = await subscribeToPanelAddTokenmaxxer(receive, failedListen, takeRequest);
    pending = true;
    await vi.advanceTimersByTimeAsync(250);

    expect(receive).toHaveBeenCalledTimes(3);
    stopFallback();
  });
});
