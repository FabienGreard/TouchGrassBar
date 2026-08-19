import { describe, expect, test, vi } from "vitest";

import {
  createSettingsDelivery,
  type SettingsPort,
  type SettingsPortOutcome,
} from "@/native-state/settings-delivery";

const settingsState = {
  contractVersion: 4,
  displayName: "Fabien",
  launchAtLogin: { availability: "available", enabled: false },
  profileProvisioning: "profile-pending",
  providers: [
    {
      displayName: "Codex",
      enabled: true,
      provider: "codex",
      status: "detected",
    },
    {
      displayName: "Claude",
      enabled: false,
      provider: "claude",
      status: "not-detected",
    },
  ],
  section: "general",
} as const;

const fakeRecoveryKey = "2".repeat(48);
const recoveryCredentials = {
  recoveryKey: fakeRecoveryKey,
  touchGrassId: "TG-234567",
};

function ignoreNavigation(_payload: unknown) {}
function ignoreRecoveryClear() {}

function port(): SettingsPort & {
  clearRecovery: () => void;
  navigate: (payload: unknown) => void;
} {
  let navigate = ignoreNavigation;
  let clearRecovery = ignoreRecoveryClear;
  return {
    clearRecovery: () => clearRecovery(),
    hide: vi.fn(async () => ({ ok: true as const, value: undefined })),
    navigate: (payload) => navigate(payload),
    read: vi.fn(async () => ({ ok: true as const, value: settingsState })),
    recoverProfile: vi.fn(async () => ({
      ok: true as const,
      value: undefined,
    })),
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
    updateDisplayName: vi.fn(async (displayName) => ({
      ok: true as const,
      value: {
        ...settingsState,
        displayName,
        profileProvisioning: "ready" as const,
        touchGrassId: "TG-234567",
      },
    })),
    setProviderEnabled: vi.fn(async (provider, enabled) => ({
      ok: true as const,
      value: {
        ...settingsState,
        providers: settingsState.providers.map((item) =>
          item.provider === provider ? { ...item, enabled } : item,
        ),
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
  test("recovers through native custody and refreshes the Profile", async () => {
    const native = port();
    native.read = vi.fn(async () => ({
      ok: true as const,
      value: {
        ...settingsState,
        displayName: "Recovered",
        profileProvisioning: "ready" as const,
        section: "profile" as const,
        touchGrassId: "TG-234567",
      },
    }));
    const delivery = createSettingsDelivery(native);

    expect(await delivery.recoverProfile(recoveryCredentials)).toBe(true);
    expect(native.recoverProfile).toHaveBeenCalledWith(recoveryCredentials);
    expect(delivery.getSnapshot().snapshot).toMatchObject({
      displayName: "Recovered",
      touchGrassId: "TG-234567",
    });
  });

  test("contains recovery failure details behind one delivery state", async () => {
    const native = port();
    native.recoverProfile = vi.fn(async () => ({
      fault: { code: "profile-recovery-unavailable" as const },
      ok: false as const,
    }));
    const delivery = createSettingsDelivery(native);

    expect(await delivery.recoverProfile(recoveryCredentials)).toBe(false);
    expect(delivery.getSnapshot()).toMatchObject({
      phase: "degraded",
      recoveryFailed: true,
    });
  });


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

  test("shows a Display Name before the native confirmation", async () => {
    const native = port();
    let confirmUpdate!: (outcome: SettingsPortOutcome<unknown>) => void;
    native.updateDisplayName = vi.fn(
      () =>
        new Promise<SettingsPortOutcome<unknown>>((resolve) => {
          confirmUpdate = resolve;
        }),
    );
    const delivery = createSettingsDelivery(native);
    await delivery.activate();

    const save = delivery.updateDisplayName("New name");
    expect(delivery.getSnapshot().snapshot).toMatchObject({
      displayName: "New name",
      profileProvisioning: "profile-pending",
    });
    expect(native.updateDisplayName).toHaveBeenCalledWith("New name");
    confirmUpdate({
      ok: true,
      value: {
        ...settingsState,
        displayName: "New name",
        profileProvisioning: "ready",
        touchGrassId: "TG-234567",
      },
    });
    expect(await save).toBe(true);
    expect(delivery.getSnapshot().snapshot).toMatchObject({
      displayName: "New name",
      profileProvisioning: "ready",
      touchGrassId: "TG-234567",
    });
  });

  test("restores the Display Name when the native update fails", async () => {
    const native = port();
    let rejectUpdate!: (outcome: SettingsPortOutcome<unknown>) => void;
    native.updateDisplayName = vi.fn(
      () =>
        new Promise<SettingsPortOutcome<unknown>>((resolve) => {
          rejectUpdate = resolve;
        }),
    );
    const delivery = createSettingsDelivery(native);
    await delivery.activate();

    const save = delivery.updateDisplayName("New name");
    expect(delivery.getSnapshot().snapshot?.displayName).toBe("New name");

    rejectUpdate({
      fault: { code: "display-name-update-unavailable" },
      ok: false,
    });
    expect(await save).toBe(false);
    expect(delivery.getSnapshot().snapshot?.displayName).toBe("Fabien");
  });

  test("commits one provider toggle only after native confirmation", async () => {
    const native = port();
    let confirm!: (outcome: SettingsPortOutcome<unknown>) => void;
    native.setProviderEnabled = vi.fn(
      () =>
        new Promise<SettingsPortOutcome<unknown>>((resolve) => {
          confirm = resolve;
        }),
    );
    const delivery = createSettingsDelivery(native);
    await delivery.activate();

    const save = delivery.setProviderEnabled("claude", true);
    expect(native.setProviderEnabled).toHaveBeenCalledWith("claude", true);
    expect(delivery.getSnapshot()).toMatchObject({
      savingProviders: ["claude"],
      snapshot: {
        providers: [
          { enabled: true, provider: "codex" },
          { enabled: false, provider: "claude" },
        ],
      },
    });

    confirm({
      ok: true,
      value: {
        ...settingsState,
        providers: settingsState.providers.map((provider) =>
          provider.provider === "claude" ? { ...provider, enabled: true } : provider,
        ),
      },
    });

    expect(await save).toBe(true);
    expect(delivery.getSnapshot()).toMatchObject({
      savingProviders: [],
      snapshot: {
        providers: [
          { enabled: true, provider: "codex" },
          { enabled: true, provider: "claude" },
        ],
      },
    });
  });

  test("keeps both providers enabled when their confirmations finish out of order", async () => {
    const native = port();
    const bothDisabled = {
      ...settingsState,
      providers: settingsState.providers.map((provider) => ({
        ...provider,
        enabled: false,
      })),
    };
    native.read = vi.fn(async () => ({
      ok: true as const,
      value: bothDisabled,
    }));
    const confirmations = new Map<
      "claude" | "codex",
      (outcome: SettingsPortOutcome<unknown>) => void
    >();
    native.setProviderEnabled = vi.fn(
      (provider) =>
        new Promise<SettingsPortOutcome<unknown>>((resolve) => {
          confirmations.set(provider, resolve);
        }),
    );
    const confirmedProviders = (confirmed: "claude" | "codex") => {
      const providers = [];
      for (const provider of bothDisabled.providers) {
        providers.push(provider.provider === confirmed ? { ...provider, enabled: true } : provider);
      }
      return providers;
    };
    const delivery = createSettingsDelivery(native);
    await delivery.activate();

    const enableCodex = delivery.setProviderEnabled("codex", true);
    const enableClaude = delivery.setProviderEnabled("claude", true);
    confirmations.get("claude")?.({
      ok: true,
      value: {
        ...bothDisabled,
        providers: confirmedProviders("claude"),
      },
    });
    expect(await enableClaude).toBe(true);
    confirmations.get("codex")?.({
      ok: true,
      value: {
        ...bothDisabled,
        providers: confirmedProviders("codex"),
      },
    });
    expect(await enableCodex).toBe(true);

    expect(delivery.getSnapshot().snapshot?.providers).toEqual([
      expect.objectContaining({ enabled: true, provider: "codex" }),
      expect.objectContaining({ enabled: true, provider: "claude" }),
    ]);
  });

  test("preserves the last confirmed provider value when a mutation fails", async () => {
    const native = port();
    native.setProviderEnabled = vi.fn(async () => ({
      fault: { code: "provider-setting-unavailable" as const },
      ok: false as const,
    }));
    const delivery = createSettingsDelivery(native);
    await delivery.activate();

    expect(await delivery.setProviderEnabled("codex", false)).toBe(false);
    expect(delivery.getSnapshot()).toMatchObject({
      phase: "degraded",
      savingProviders: [],
      snapshot: {
        providers: [
          { enabled: true, provider: "codex" },
          { enabled: false, provider: "claude" },
        ],
      },
    });
  });

  test("rejects a native response that does not confirm the provider value", async () => {
    const native = port();
    native.setProviderEnabled = vi.fn(async () => ({
      ok: true as const,
      value: settingsState,
    }));
    const delivery = createSettingsDelivery(native);
    await delivery.activate();

    expect(await delivery.setProviderEnabled("codex", false)).toBe(false);
    expect(delivery.getSnapshot()).toMatchObject({
      phase: "degraded",
      savingProviders: [],
      snapshot: {
        providers: [
          { enabled: true, provider: "codex" },
          { enabled: false, provider: "claude" },
        ],
      },
    });
  });

  test("does not let an older read replace a confirmed provider value", async () => {
    const native = port();
    const delivery = createSettingsDelivery(native);
    await delivery.activate();
    let finishRead!: (outcome: SettingsPortOutcome<unknown>) => void;
    native.read = vi.fn(
      () =>
        new Promise<SettingsPortOutcome<unknown>>((resolve) => {
          finishRead = resolve;
        }),
    );

    const olderRead = delivery.read();
    expect(await delivery.setProviderEnabled("claude", true)).toBe(true);
    finishRead({ ok: true, value: settingsState });
    await olderRead;

    expect(delivery.getSnapshot().snapshot?.providers).toEqual([
      expect.objectContaining({ enabled: true, provider: "codex" }),
      expect.objectContaining({ enabled: true, provider: "claude" }),
    ]);
  });

  test("merges an older launch response with a confirmed provider value", async () => {
    const native = port();
    const delivery = createSettingsDelivery(native);
    await delivery.activate();
    let finishLaunch!: (outcome: SettingsPortOutcome<unknown>) => void;
    native.setLaunchAtLogin = vi.fn(
      () =>
        new Promise<SettingsPortOutcome<unknown>>((resolve) => {
          finishLaunch = resolve;
        }),
    );

    const launch = delivery.setLaunchAtLogin(true);
    expect(await delivery.setProviderEnabled("claude", true)).toBe(true);
    finishLaunch({
      ok: true,
      value: {
        ...settingsState,
        launchAtLogin: { availability: "available", enabled: true },
      },
    });
    expect(await launch).toBe(true);

    expect(delivery.getSnapshot()).toMatchObject({
      snapshot: {
        launchAtLogin: { availability: "available", enabled: true },
        providers: [
          { enabled: true, provider: "codex" },
          { enabled: true, provider: "claude" },
        ],
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

  test("stale activation disposal cannot disable the current clear stream", async () => {
    const native = port();
    const subscriptions: Array<{
      resolve: (outcome: SettingsPortOutcome<() => void>) => void;
      stop: ReturnType<typeof vi.fn>;
    }> = [];
    native.subscribeRecoveryClear = vi.fn(
      () =>
        new Promise<SettingsPortOutcome<() => void>>((resolve) => {
          subscriptions.push({ resolve, stop: vi.fn() });
        }),
    );
    const delivery = createSettingsDelivery(native);

    const staleActivation = delivery.activate();
    const currentActivation = delivery.activate();
    await vi.waitFor(() => expect(subscriptions).toHaveLength(2));
    subscriptions[1]!.resolve({
      ok: true,
      value: subscriptions[1]!.stop as () => void,
    });
    const disposeCurrent = await currentActivation;
    subscriptions[0]!.resolve({
      ok: true,
      value: subscriptions[0]!.stop as () => void,
    });
    const disposeStale = await staleActivation;

    disposeStale();
    expect(subscriptions[0]!.stop).toHaveBeenCalledOnce();
    expect(await delivery.revealRecoveryKey()).toBe(true);

    disposeCurrent();
    expect(subscriptions[1]!.stop).toHaveBeenCalledOnce();
    expect(await delivery.revealRecoveryKey()).toBe(false);
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
