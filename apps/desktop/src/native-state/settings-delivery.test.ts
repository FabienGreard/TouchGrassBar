import { describe, expect, test, vi } from "vitest";

import {
  createSettingsDelivery,
  type SettingsPort,
  type SettingsPortOutcome,
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

const fakeRecoveryKey = "2".repeat(48);

function ignoreNavigation(_payload: unknown) {}

function port(): SettingsPort & {
  clearRecovery: () => void;
  navigate: (payload: unknown) => void;
} {
  let navigate = ignoreNavigation;
  let clearRecovery = () => undefined;
  return {
    clearRecovery: () => clearRecovery(),
    hide: vi.fn(async () => ({ ok: true as const, value: undefined })),
    navigate: (payload) => navigate(payload),
    read: vi.fn(async () => ({ ok: true as const, value: settingsState })),
    revealRecoveryKey: vi.fn(async () => ({
      ok: true as const,
      value: fakeRecoveryKey,
    })),
    selectSection: vi.fn(async () => ({
      ok: true as const,
      value: undefined,
    })),
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
    subscribeRecoveryClear: vi.fn(async (receive) => {
      clearRecovery = receive;
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
    expect(native.selectSection).not.toHaveBeenCalled();
    native.navigate({ section: "providers", localPath: "/private" });
    expect(delivery.getSnapshot().snapshot?.section).toBe("profile");
  });

  test("refreshes the hidden Settings snapshot when native navigation activates it", async () => {
    const native = port();
    vi.mocked(native.read)
      .mockResolvedValueOnce({
        ok: true as const,
        value: {
          ...settingsState,
          displayName: undefined,
          profileProvisioning: "not-authorized",
        },
      })
      .mockResolvedValueOnce({ ok: true as const, value: settingsState });
    const delivery = createSettingsDelivery(native);
    await delivery.activate();

    native.navigate({ section: "profile" });

    await vi.waitFor(() => {
      expect(delivery.getSnapshot()).toMatchObject({
        phase: "ready",
        snapshot: {
          displayName: "Fabien",
          profileProvisioning: "profile-pending",
          section: "profile",
        },
      });
    });
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

    expect(native.selectSection).toHaveBeenCalledWith("providers");
    expect(delivery.getSnapshot().snapshot?.section).toBe("providers");
  });

  test("keeps the deliberate Recovery Key reveal only until it is hidden", async () => {
    const native = port();
    let confirmSection!: (outcome: SettingsPortOutcome<void>) => void;
    native.selectSection = vi.fn(
      () =>
        new Promise<SettingsPortOutcome<void>>((resolve) => {
          confirmSection = resolve;
        }),
    );
    const delivery = createSettingsDelivery(native);
    await delivery.activate();

    delivery.selectSection("profile");
    const reveal = delivery.revealRecoveryKey();
    await Promise.resolve();
    expect(native.revealRecoveryKey).not.toHaveBeenCalled();

    confirmSection({ ok: true, value: undefined });
    expect(await reveal).toBe(true);
    expect(native.selectSection).toHaveBeenCalledWith("profile");
    expect(native.revealRecoveryKey).toHaveBeenCalledOnce();
    expect(delivery.getSnapshot()).toMatchObject({
      recoveryKey: fakeRecoveryKey,
      revealingRecoveryKey: false,
    });
    expect(await delivery.hideRecoveryKey()).toBe(true);
    expect(delivery.getSnapshot()).toMatchObject({ recoveryKey: null });
  });

  test("does not disclose Recovery Key for a superseded Profile selection", async () => {
    const native = port();
    let confirmProfile!: (outcome: SettingsPortOutcome<void>) => void;
    native.selectSection = vi.fn((section) => {
      if (section !== "profile") {
        return Promise.resolve({ ok: true as const, value: undefined });
      }
      return new Promise<SettingsPortOutcome<void>>((resolve) => {
        confirmProfile = resolve;
      });
    });
    const delivery = createSettingsDelivery(native);
    await delivery.activate();

    delivery.selectSection("profile");
    const staleReveal = delivery.revealRecoveryKey();
    await Promise.resolve();
    delivery.selectSection("general");
    confirmProfile({ ok: true, value: undefined });
    expect(await staleReveal).toBe(false);
    await Promise.resolve();
    await vi.waitFor(() => {
      expect(native.selectSection).toHaveBeenCalledWith("general");
    });
    expect(native.revealRecoveryKey).not.toHaveBeenCalled();
  });

  test("native window clear invalidates visible and in-flight Recovery Keys", async () => {
    const native = port();
    let finishReveal!: (outcome: SettingsPortOutcome<string>) => void;
    native.revealRecoveryKey = vi.fn(
      () =>
        new Promise<SettingsPortOutcome<string>>((resolve) => {
          finishReveal = resolve;
        }),
    );
    const delivery = createSettingsDelivery(native);
    await delivery.activate();

    const reveal = delivery.revealRecoveryKey();
    await Promise.resolve();
    expect(delivery.getSnapshot().revealingRecoveryKey).toBe(true);
    native.clearRecovery();
    expect(delivery.getSnapshot()).toMatchObject({
      recoveryKey: null,
      revealingRecoveryKey: false,
    });
    finishReveal({ ok: true, value: fakeRecoveryKey });
    expect(await reveal).toBe(false);
    expect(delivery.getSnapshot().recoveryKey).toBeNull();
  });

  test("fails closed when the native recovery-clear stream is unavailable", async () => {
    const native = port();
    native.read = vi.fn(async () => ({
      ok: true as const,
      value: {
        ...settingsState,
        profileProvisioning: "ready",
        recoveryKeySuffix: "K9m",
        touchGrassId: "TG-234567",
      },
    }));
    native.subscribeRecoveryClear = vi.fn(async () => ({
      fault: { code: "recovery-clear-stream-unavailable" as const },
      ok: false as const,
    }));
    const delivery = createSettingsDelivery(native);

    await delivery.activate();

    expect(delivery.getSnapshot()).toMatchObject({
      phase: "degraded",
      snapshot: { recoveryKeySuffix: null },
    });
    expect(await delivery.revealRecoveryKey()).toBe(false);
    expect(native.revealRecoveryKey).not.toHaveBeenCalled();
  });

  test("a changed Profile recovery context clears the revealed key", async () => {
    const native = port();
    native.read = vi
      .fn()
      .mockResolvedValueOnce({
        ok: true as const,
        value: {
          ...settingsState,
          profileProvisioning: "ready",
          recoveryKeySuffix: "K9m",
          touchGrassId: "TG-234567",
        },
      })
      .mockResolvedValueOnce({
        ok: true as const,
        value: {
          ...settingsState,
          profileProvisioning: "ready",
          recoveryKeySuffix: "P7x",
          touchGrassId: "TG-765432",
        },
      });
    const delivery = createSettingsDelivery(native);
    await delivery.activate();
    expect(await delivery.revealRecoveryKey()).toBe(true);

    await delivery.read();

    expect(delivery.getSnapshot()).toMatchObject({
      recoveryKey: null,
      snapshot: {
        recoveryKeySuffix: "P7x",
        touchGrassId: "TG-765432",
      },
    });
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
