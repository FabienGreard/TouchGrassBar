import { describe, expect, test, vi } from "vitest";

import {
  bindRecoveryKeyClearEvents,
  focusAndSelectRecoveryInput,
  maskRecoveryKeySuffix,
  RECOVERY_KEY_PLACEHOLDER,
} from "./recovery-key-input";

describe("Recovery Key input", () => {
  test("focuses without scrolling and selects the full input value", () => {
    const input = {
      focus: vi.fn(),
      select: vi.fn(),
    };

    focusAndSelectRecoveryInput(input);

    expect(input.focus).toHaveBeenCalledWith({ preventScroll: true });
    expect(input.select).toHaveBeenCalledOnce();
    expect(maskRecoveryKeySuffix("K9m")).toBe("••••••••••••K9m");
    expect(RECOVERY_KEY_PLACEHOLDER).toBe("••••••••••••••••");
    expect(maskRecoveryKeySuffix("K9m")).not.toContain("-");
  });

  test("clears on resize and captured scroll until cleanup", () => {
    const listeners = new Map<string, () => void>();
    const target = {
      addEventListener: vi.fn((event: string, listener: () => void) =>
        listeners.set(event, listener),
      ),
      removeEventListener: vi.fn((event: string) => listeners.delete(event)),
    } as unknown as Pick<Window, "addEventListener" | "removeEventListener">;
    const clear = vi.fn();

    const cleanup = bindRecoveryKeyClearEvents(target, clear);
    listeners.get("resize")?.();
    listeners.get("scroll")?.();

    expect(clear).toHaveBeenCalledTimes(2);
    expect(target.addEventListener).toHaveBeenCalledWith("scroll", clear, true);
    cleanup();
    expect(target.removeEventListener).toHaveBeenCalledWith("scroll", clear, true);
  });
});
