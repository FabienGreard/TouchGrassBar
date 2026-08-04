import { describe, expect, test, vi } from "vitest";

import { createNativeWindowKeyboardHandler } from "./native-window-keyboard";

describe("native Settings and onboarding keyboard contract", () => {
  test.each([
    { key: "Escape", metaKey: false },
    { key: "w", metaKey: true },
    { key: "W", metaKey: true },
  ])("dismisses $key with meta=$metaKey", ({ key, metaKey }) => {
    const hide = vi.fn();
    const preventDefault = vi.fn();
    const handler = createNativeWindowKeyboardHandler({ enabled: true, hide });

    handler({ key, metaKey, preventDefault });

    expect(preventDefault).toHaveBeenCalledOnce();
    expect(hide).toHaveBeenCalledOnce();
  });

  test("ignores plain letters and every shortcut while disabled", () => {
    const hide = vi.fn();
    const preventDefault = vi.fn();
    const enabled = createNativeWindowKeyboardHandler({ enabled: true, hide });
    const disabled = createNativeWindowKeyboardHandler({
      enabled: false,
      hide,
    });

    enabled({ key: "w", metaKey: false, preventDefault });
    disabled({ key: "Escape", metaKey: false, preventDefault });

    expect(preventDefault).not.toHaveBeenCalled();
    expect(hide).not.toHaveBeenCalled();
  });
});
