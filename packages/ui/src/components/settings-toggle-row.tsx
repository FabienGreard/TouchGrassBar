import { Switch } from "./switch";

function SettingsToggleRow({
  checked,
  description,
  disabled = false,
  grouped = false,
  label,
  onCheckedChange,
}: {
  checked: boolean;
  description?: string;
  disabled?: boolean;
  grouped?: boolean;
  label: string;
  onCheckedChange?: ((checked: boolean) => void) | undefined;
}) {
  return (
    <label
      className={`grid cursor-pointer grid-cols-[1fr_auto] items-center gap-8 px-4 py-3.5 ${grouped ? "border-t border-sheet-row-border" : "rounded-[12px] border border-sheet-row-border bg-sheet-row"}`}
      data-grouped={grouped || undefined}
      data-slot="settings-toggle-row"
    >
      <span>
        <strong className="block text-[12px]">{label}</strong>
        {description ? (
          <small className="mt-0.5 block text-[9px] text-sheet-muted">
            {description}
          </small>
        ) : null}
      </span>
      <Switch
        aria-label={label}
        checked={checked}
        disabled={disabled || onCheckedChange === undefined}
        {...(onCheckedChange === undefined ? {} : { onCheckedChange })}
      />
    </label>
  );
}

export { SettingsToggleRow };
