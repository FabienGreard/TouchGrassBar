type NativeWindowKeyboardEvent = Pick<
  KeyboardEvent,
  "defaultPrevented" | "key" | "metaKey" | "preventDefault"
>;

function isNativeWindowDismissal(event: Pick<NativeWindowKeyboardEvent, "key" | "metaKey">) {
  return event.key === "Escape" || (event.metaKey && event.key.toLowerCase() === "w");
}

function createNativeWindowKeyboardHandler({
  enabled,
  hide,
}: {
  enabled: boolean;
  hide: () => void;
}) {
  return (event: NativeWindowKeyboardEvent) => {
    if (event.defaultPrevented || !enabled || !isNativeWindowDismissal(event)) {
      return;
    }
    event.preventDefault();
    hide();
  };
}

export { createNativeWindowKeyboardHandler, isNativeWindowDismissal };
