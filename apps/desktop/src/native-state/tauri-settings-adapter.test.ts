import { describe, expect, test, vi } from "vitest";

import {
  createTauriSettingsAdapter,
  type TauriSettingsBindings,
} from "@/native-state/tauri-settings-adapter";

describe("Tauri Settings adapter", () => {
  test("uses exact Settings commands and the bounded navigation event", async () => {
    const fakeRecoveryKey = "2".repeat(48);
    const stops = new Map<string, ReturnType<typeof vi.fn>>();
    const listeners = new Map<
      string,
      (event: { payload: unknown }) => void
    >();
    const bindings: TauriSettingsBindings = {
      invoke: vi.fn(async (command) =>
        command === "reveal_recovery_key" ? fakeRecoveryKey : { command },
      ),
      listen: vi.fn(async (event, receive) => {
        listeners.set(event, receive);
        const stop = vi.fn();
        stops.set(event, stop);
        return stop;
      }),
    };
    const adapter = createTauriSettingsAdapter(bindings);
    const receive = vi.fn();
    const clearRecovery = vi.fn();

    await adapter.read();
    await adapter.setLaunchAtLogin(true);
    await adapter.updateDisplayName("New name");
    await adapter.setProviderEnabled("claude", false);
    await adapter.selectSection("profile");
    expect(await adapter.revealRecoveryKey()).toEqual({
      ok: true,
      value: fakeRecoveryKey,
    });
    const subscription = await adapter.subscribeNavigation(receive);
    const recoverySubscription =
      await adapter.subscribeRecoveryClear(clearRecovery);
    listeners.get("settings-navigation-requested")?.({
      payload: { section: "profile" },
    });
    listeners.get("settings-recovery-clear-requested")?.({
      payload: null,
    });

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
      "update_profile_display_name",
      { displayName: "New name" },
    );
    expect(bindings.invoke).toHaveBeenNthCalledWith(
      4,
      "set_provider_enabled",
      { enabled: false, provider: "claude" },
    );
    expect(bindings.invoke).toHaveBeenNthCalledWith(
      5,
      "select_settings_section",
      { section: "profile" },
    );
    expect(bindings.invoke).toHaveBeenNthCalledWith(
      6,
      "reveal_recovery_key",
      undefined,
    );
    expect(bindings.listen).toHaveBeenCalledWith(
      "settings-navigation-requested",
      expect.any(Function),
    );
    expect(bindings.listen).toHaveBeenCalledWith(
      "settings-recovery-clear-requested",
      expect.any(Function),
    );
    expect(receive).toHaveBeenCalledWith({ section: "profile" });
    expect(clearRecovery).toHaveBeenCalledOnce();
    if (subscription.ok) subscription.value();
    if (recoverySubscription.ok) recoverySubscription.value();
    expect(stops.get("settings-navigation-requested")).toHaveBeenCalledOnce();
    expect(
      stops.get("settings-recovery-clear-requested"),
    ).toHaveBeenCalledOnce();
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
    expect(await adapter.updateDisplayName("New name")).toEqual({
      fault: { code: "display-name-update-unavailable" },
      ok: false,
    });
    expect(await adapter.setProviderEnabled("codex", false)).toEqual({
      fault: { code: "provider-setting-unavailable" },
      ok: false,
    });
    expect(await adapter.selectSection("profile")).toEqual({
      fault: { code: "settings-section-unavailable" },
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
    expect(await adapter.subscribeRecoveryClear(() => undefined)).toEqual({
      fault: { code: "recovery-clear-stream-unavailable" },
      ok: false,
    });
  });

  test("rejects a malformed Recovery Key response", async () => {
    const adapter = createTauriSettingsAdapter({
      invoke: vi.fn(async () => "not-a-recovery-key"),
      listen: vi.fn(async () => () => undefined),
    });

    expect(await adapter.revealRecoveryKey()).toEqual({
      fault: { code: "recovery-key-unavailable" },
      ok: false,
    });
  });
});
