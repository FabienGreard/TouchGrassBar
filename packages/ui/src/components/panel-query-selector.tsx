import { Button } from "./button";
import {
  PanelMenu,
  PanelMenuContent,
  PanelMenuRadioGroup,
  PanelMenuRadioItem,
  PanelMenuTrigger,
} from "./panel-menu";
type QueryOption = { label: string; value: string };
function PanelQuerySelector({
  label,
  onIntent = () => undefined,
  onValueChange,
  options,
  value,
}: {
  label: string;
  onIntent?: ((value: string) => void) | undefined;
  onValueChange: (value: string) => void;
  options: readonly QueryOption[];
  value: string;
}) {
  const selectedLabel = options.find((option) => option.value === value)?.label ?? value;
  const announceAlternativeIntents = () => {
    for (const option of options) {
      if (option.value !== value) onIntent(option.value);
    }
  };

  return (
    <PanelMenu>
      <PanelMenuTrigger asChild>
        <Button
          aria-label={`Select ${label}`}
          onFocus={announceAlternativeIntents}
          onPointerEnter={announceAlternativeIntents}
          size="quiet"
          type="button"
          variant="ghost"
        >
          {selectedLabel}
        </Button>
      </PanelMenuTrigger>
      <PanelMenuContent align="end" side="top" sideOffset={9} size="compact">
        <PanelMenuRadioGroup onValueChange={onValueChange} value={value}>
          {options.map((option) => (
            <PanelMenuRadioItem key={option.value} value={option.value}>
              {option.label}
            </PanelMenuRadioItem>
          ))}
        </PanelMenuRadioGroup>
      </PanelMenuContent>
    </PanelMenu>
  );
}

export { PanelQuerySelector };
