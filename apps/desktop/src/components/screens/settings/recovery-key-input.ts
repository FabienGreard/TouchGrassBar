type SelectableRecoveryInput = Pick<HTMLInputElement, "focus" | "select">;

const RECOVERY_KEY_PLACEHOLDER = "••••••••••••••••";

function maskRecoveryKeySuffix(suffix: string | null) {
  return suffix === null ? RECOVERY_KEY_PLACEHOLDER : `••••••••••••${suffix}`;
}

function focusAndSelectRecoveryInput(input: SelectableRecoveryInput | null) {
  if (input === null) return;
  input.focus({ preventScroll: true });
  input.select();
}

function bindRecoveryKeyClearEvents(
  target: Pick<Window, "addEventListener" | "removeEventListener">,
  clear: () => void,
) {
  target.addEventListener("resize", clear);
  target.addEventListener("scroll", clear, true);
  return () => {
    target.removeEventListener("resize", clear);
    target.removeEventListener("scroll", clear, true);
  };
}

export {
  bindRecoveryKeyClearEvents,
  focusAndSelectRecoveryInput,
  maskRecoveryKeySuffix,
  RECOVERY_KEY_PLACEHOLDER,
};
export type { SelectableRecoveryInput };
