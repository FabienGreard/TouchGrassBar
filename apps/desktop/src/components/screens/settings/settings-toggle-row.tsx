import { Switch } from "@touchgrass/ui";

function SettingsToggleRow({
  checked,
  description,
  disabled = false,
  label,
  onCheckedChange,
}: {
  checked: boolean;
  description?: string;
  disabled?: boolean;
  label: string;
  onCheckedChange?: ((checked: boolean) => void) | undefined;
}) {
  return (
    <label
      className="grid cursor-pointer grid-cols-[1fr_auto] items-center gap-8 rounded-[12px] bg-white/38 px-4 py-3.5"
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
