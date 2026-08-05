import { describe, expect, test, vi } from "vitest";

import {
  createSettingsDelivery,
  type SettingsPort,
} from "@/native-state/settings-delivery";

const settingsState = {
  contractVersion: 2,
  displayName: "Fabien",
  launchAtLogin: { availability: "available", enabled: false },
  profileProvisioning: "profile-pending",
  providers: [
    { provider: "codex", status: "detected" },
    { provider: "claude", status: "not-detected" },
  ],
  section: "general",
} as const;

function ignoreNavigation(_payload: unknown) {}

function port(): SettingsPort & { navigate: (payload: unknown) => void } {
  let navigate = ignoreNavigation;
  return {
    hide: vi.fn(async () => ({ ok: true as const, value: undefined })),
    navigate: (payload) => navigate(payload),
    read: vi.fn(async () => ({ ok: true as const, value: settingsState })),
    setLaunchAtLogin: vi.fn(async (enabled) => ({
      ok: true as const,
      value: {
        ...settingsState,
        launchAtLogin: { availability: "available", enabled },
      },
    })),
    subscribeNavigation: vi.fn(async (receive) => {
      navigate = receive;
      return { ok: true as const, value: () => undefined };
    }),
  };
}

describe("Settings delivery", () => {
  test("subscribes before loading and accepts bounded native navigation", async () => {
    const native = port();
    const delivery = createSettingsDelivery(native);
    await delivery.activate();

    native.navigate({ section: "profile" });
    expect(delivery.getSnapshot()).toMatchObject({
      phase: "ready",
      snapshot: { section: "profile" },
    });
    native.navigate({ section: "providers", localPath: "/private" });
    expect(delivery.getSnapshot().snapshot?.section).toBe("profile");
  });

  test("commits launch-at-login only after the native confirmation", async () => {
    const native = port();
    const delivery = createSettingsDelivery(native);
    await delivery.activate();

    expect(await delivery.setLaunchAtLogin(true)).toBe(true);
    expect(native.setLaunchAtLogin).toHaveBeenCalledWith(true);
    expect(delivery.getSnapshot()).toMatchObject({
      savingLaunchAtLogin: false,
      snapshot: {
        launchAtLogin: { availability: "available", enabled: true },
      },
    });
  });

  test("preserves the selected section across provider refreshes", async () => {
    const native = port();
    const delivery = createSettingsDelivery(native);
    await delivery.activate();

    delivery.selectSection("providers");
    await delivery.read();

    expect(delivery.getSnapshot().snapshot?.section).toBe("providers");
  });

  test("preserves the last confirmed toggle when a mutation fails", async () => {
    const native = port();
    native.setLaunchAtLogin = vi.fn(async () => ({
      fault: { code: "launch-at-login-unavailable" as const },
      ok: false as const,
    }));
    const delivery = createSettingsDelivery(native);
    await delivery.activate();

    expect(await delivery.setLaunchAtLogin(true)).toBe(false);
    expect(delivery.getSnapshot()).toMatchObject({
      phase: "degraded",
      savingLaunchAtLogin: false,
      snapshot: {
        launchAtLogin: { availability: "available", enabled: false },
      },
    });
  });
});
