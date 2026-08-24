import { describe, expect, test, vi } from "vitest";

import { createBootstrapDelivery, type BootstrapPort } from "@/native-state/bootstrap-delivery";

const bootstrapState = {
  bootstrap: "required",
  contractVersion: 3,
  displayName: null,
  persistence: "available",
  profileProvisioning: "not-authorized",
  providers: [
    { displayName: "Codex", provider: "codex", status: "detected" },
    { displayName: "Claude", provider: "claude", status: "not-detected" },
  ],
} as const;

const recoveryCredentials = {
  recoveryKey: "2".repeat(48),
  touchGrassId: "TG-234567",
};

function port(): BootstrapPort {
  return {
    complete: vi.fn(async () => ({
      ok: true as const,
      value: {
        ...bootstrapState,
        bootstrap: "completed",
        displayName: "Fabien",
        profileProvisioning: "profile-pending",
      },
    })),
    hide: vi.fn(async () => ({ ok: true as const, value: undefined })),
    read: vi.fn(async () => ({ ok: true as const, value: bootstrapState })),
    recoverProfile: vi.fn(async () => ({
      ok: true as const,
      value: undefined,
    })),
  };
}

describe("bootstrap delivery", () => {
  test("keeps onboarding visible while Profile creation is Pending", async () => {
    const native = port();
    const delivery = createBootstrapDelivery(native);

    await delivery.read();
    expect(delivery.getSnapshot()).toEqual({
      phase: "ready",
      snapshot: bootstrapState,
      submitting: false,
    });

    expect(await delivery.complete("  Fabien  ")).toBe(false);
    expect(native.complete).toHaveBeenCalledWith("Fabien");
    expect(native.hide).not.toHaveBeenCalled();
    expect(delivery.getSnapshot()).toMatchObject({
      phase: "ready",
      snapshot: {
        bootstrap: "completed",
        profileProvisioning: "profile-pending",
      },
      submitting: false,
    });
  });

  test("closes onboarding after Profile creation is Ready", async () => {
    const native = port();
    native.complete = vi.fn(async () => ({
      ok: true as const,
      value: {
        ...bootstrapState,
        bootstrap: "completed",
        displayName: "Fabien",
        profileProvisioning: "ready",
      },
    }));
    const delivery = createBootstrapDelivery(native);
    await delivery.read();

    expect(await delivery.complete("Fabien")).toBe(true);
    expect(native.hide).toHaveBeenCalledOnce();
  });

  test("coalesces duplicate completion and closes invalid or raw native shapes", async () => {
    let finish!: (value: unknown) => void;
    const completion = new Promise<unknown>((resolve) => {
      finish = resolve;
    });
    const native = port();
    native.complete = vi.fn(async () => ({
      ok: true as const,
      value: await completion,
    }));
    const delivery = createBootstrapDelivery(native);
    await delivery.read();

    const first = delivery.complete("Fabien");
    const joined = delivery.complete("Fabien");
    expect(joined).toBe(first);
    finish({ ...bootstrapState, localPath: "/private/provider" });

    expect(await first).toBe(false);
    expect(native.complete).toHaveBeenCalledOnce();
    expect(delivery.getSnapshot().phase).toBe("degraded");
  });

  test("fails closed when the completed onboarding surface cannot close", async () => {
    const native = port();
    native.complete = vi.fn(async () => ({
      ok: true as const,
      value: {
        ...bootstrapState,
        bootstrap: "completed",
        displayName: "Fabien",
        profileProvisioning: "ready",
      },
    }));
    native.hide = vi.fn(async () => ({
      fault: { code: "surface-unavailable" as const },
      ok: false as const,
    }));
    const delivery = createBootstrapDelivery(native);
    await delivery.read();

    expect(await delivery.complete("Fabien")).toBe(false);
    expect(native.hide).toHaveBeenCalledOnce();
  });

  test("recovers through native custody and closes only after Ready", async () => {
    const native = port();
    native.read = vi.fn(async () => ({
      ok: true as const,
      value: {
        ...bootstrapState,
        bootstrap: "completed" as const,
        displayName: "Recovered",
        profileProvisioning: "ready" as const,
        touchGrassId: "TG-234567",
      },
    }));
    const delivery = createBootstrapDelivery(native);

    expect(await delivery.recoverProfile(recoveryCredentials)).toBe(true);
    expect(native.recoverProfile).toHaveBeenCalledWith(recoveryCredentials);
    expect(native.hide).toHaveBeenCalledOnce();
  });
});
