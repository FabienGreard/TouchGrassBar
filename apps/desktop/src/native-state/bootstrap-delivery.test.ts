import { describe, expect, test, vi } from "vitest";

import {
  createBootstrapDelivery,
  type BootstrapPort,
} from "@/native-state/bootstrap-delivery";

const bootstrapState = {
  bootstrap: "required",
  contractVersion: 1,
  displayName: null,
  persistence: "available",
  profileProvisioning: "not-authorized",
  providers: [
    { provider: "codex", status: "detected" },
    { provider: "claude", status: "not-detected" },
  ],
} as const;

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
  };
}

describe("bootstrap delivery", () => {
  test("validates native provider presence and completes as Profile Pending", async () => {
    const native = port();
    const delivery = createBootstrapDelivery(native);

    await delivery.read();
    expect(delivery.getSnapshot()).toEqual({
      phase: "ready",
      snapshot: bootstrapState,
      submitting: false,
    });

    expect(await delivery.complete("  Fabien  ")).toBe(true);
    expect(native.complete).toHaveBeenCalledWith("Fabien");
    expect(delivery.getSnapshot()).toMatchObject({
      phase: "ready",
      snapshot: {
        bootstrap: "completed",
        profileProvisioning: "profile-pending",
      },
      submitting: false,
    });
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
});
