type PanelKeyboardCommand = "hide_panel" | "open_settings";

type PanelKeyboardHandlerOptions = {
  dispatch: (command: PanelKeyboardCommand) => void;
  enabled: boolean;
};

function resolvePanelKeyboardCommand(
  event: Pick<KeyboardEvent, "key" | "metaKey">,
): PanelKeyboardCommand | null {
  if (
    event.key === "Escape" ||
    (event.metaKey && event.key.toLowerCase() === "w")
  )
    return "hide_panel";
  if (event.metaKey && event.key === ",") return "open_settings";
  return null;
}

function createPanelKeyboardHandler({
  dispatch,
  enabled,
}: PanelKeyboardHandlerOptions) {
  return (event: Pick<KeyboardEvent, "key" | "metaKey">) => {
    if (!enabled) return;
    const command = resolvePanelKeyboardCommand(event);
    if (command !== null) dispatch(command);
  };
}

export { createPanelKeyboardHandler, resolvePanelKeyboardCommand };
export type { PanelKeyboardCommand };
