import { describe, expect, test, vi } from "vitest";

import {
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
});
