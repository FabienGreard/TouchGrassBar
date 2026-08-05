import { describe, expect, test, vi } from "vitest";

import {
  createTauriSettingsAdapter,
  type TauriSettingsBindings,
} from "@/native-state/tauri-settings-adapter";

describe("Tauri Settings adapter", () => {
  test("uses exact Settings commands and the bounded navigation event", async () => {
    const stop = vi.fn();
    let navigate!: (event: { payload: unknown }) => void;
    const bindings: TauriSettingsBindings = {
      invoke: vi.fn(async (command) => ({ command })),
      listen: vi.fn(async (_event, receive) => {
        navigate = receive;
        return stop;
      }),
    };
    const adapter = createTauriSettingsAdapter(bindings);
    const receive = vi.fn();

    await adapter.read();
    await adapter.setLaunchAtLogin(true);
    await adapter.selectSection("profile");
    await adapter.requestRecoveryDisclosure();
    await adapter.revealRecoveryKey();
    const subscription = await adapter.subscribeNavigation(receive);
    navigate({ payload: { section: "profile" } });

    expect(bindings.invoke).toHaveBeenNthCalledWith(
      1,
      "get_settings_state",
      undefined,
    );
    expect(bindings.invoke).toHaveBeenNthCalledWith(2, "set_launch_at_login", {
      enabled: true,
    });
    expect(bindings.invoke).toHaveBeenNthCalledWith(
      3,
      "select_settings_section",
      { section: "profile" },
    );
    expect(bindings.invoke).toHaveBeenNthCalledWith(
      4,
      "request_recovery_disclosure",
      undefined,
    );
    expect(bindings.invoke).toHaveBeenNthCalledWith(
      5,
      "reveal_recovery_key",
      undefined,
    );
    expect(bindings.listen).toHaveBeenCalledWith(
      "settings-navigation-requested",
      expect.any(Function),
    );
    expect(receive).toHaveBeenCalledWith({ section: "profile" });
    if (subscription.ok) subscription.value();
    expect(stop).toHaveBeenCalledOnce();
  });

  test("contains raw invoke and listener failures", async () => {
    const privateFailure = new Error("private path detail");
    const adapter = createTauriSettingsAdapter({
      invoke: vi.fn(() => Promise.reject(privateFailure)),
      listen: vi.fn(() => Promise.reject(privateFailure)),
    });

    expect(await adapter.read()).toEqual({
      fault: { code: "settings-state-unavailable" },
      ok: false,
    });
    expect(await adapter.setLaunchAtLogin(true)).toEqual({
      fault: { code: "launch-at-login-unavailable" },
      ok: false,
    });
    expect(await adapter.selectSection("profile")).toEqual({
      fault: { code: "settings-section-unavailable" },
      ok: false,
    });
    expect(await adapter.requestRecoveryDisclosure()).toEqual({
      fault: { code: "recovery-key-unavailable" },
      ok: false,
    });
    expect(await adapter.revealRecoveryKey()).toEqual({
      fault: { code: "recovery-key-unavailable" },
      ok: false,
    });
    expect(await adapter.subscribeNavigation(() => undefined)).toEqual({
      fault: { code: "navigation-stream-unavailable" },
      ok: false,
    });
  });
});
