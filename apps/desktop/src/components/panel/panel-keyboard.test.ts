import { describe, expect, test } from "vitest";

import { createPanelKeyboardHandler } from "@/components/panel/panel-keyboard";

describe("panel shortcut mapping used by PanelScreen", () => {
  test("dispatches only supported shortcuts while native handling is enabled", () => {
    const commands: string[] = [];
    const onKeyDown = createPanelKeyboardHandler({
      dispatch: (command) => commands.push(command),
      enabled: true,
    });

    onKeyDown({ key: "Escape", metaKey: false });
    onKeyDown({ key: "w", metaKey: true });
    onKeyDown({ key: "W", metaKey: true });
    onKeyDown({ key: ",", metaKey: true });
    onKeyDown({ key: "w", metaKey: false });
    onKeyDown({ key: "x", metaKey: true });

    expect(commands).toEqual(["hide_panel", "hide_panel", "hide_panel", "open_settings"]);
  });

  test("does not dispatch browser-preview shortcuts", () => {
    const commands: string[] = [];
    const onKeyDown = createPanelKeyboardHandler({
      dispatch: (command) => commands.push(command),
      enabled: false,
    });

    onKeyDown({ key: "Escape", metaKey: false });
    onKeyDown({ key: "w", metaKey: true });
    onKeyDown({ key: ",", metaKey: true });

    expect(commands).toEqual([]);
  });
});
