import { PANEL_INVITE_EVENT } from "@touchgrass/contracts";
import { describe, expect, test, vi } from "vitest";

import { subscribeToPanelInvite } from "./panel-invite";

describe("panel invite requests", () => {
  test("maps the native tray request to the existing panel interaction", async () => {
    const stop = vi.fn();
    let requestInvite!: () => void;
    const listen = vi.fn(async (event: string, receive: () => void) => {
      expect(event).toBe(PANEL_INVITE_EVENT);
      requestInvite = receive;
      return stop;
    });
    const receive = vi.fn();

    const unsubscribe = await subscribeToPanelInvite(receive, listen);
    requestInvite();

    expect(receive).toHaveBeenCalledOnce();
    unsubscribe();
    expect(stop).toHaveBeenCalledOnce();
  });
});
