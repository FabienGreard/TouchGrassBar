import type { ComponentProps } from "react";

import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from "../internal/dropdown-menu";
import { cn } from "#lib/utils";

function PanelMenu(props: ComponentProps<typeof DropdownMenu>) {
  return <DropdownMenu {...props} />;
}

function PanelMenuTrigger(props: ComponentProps<typeof DropdownMenuTrigger>) {
  return <DropdownMenuTrigger {...props} />;
}

function PanelMenuRadioGroup(props: ComponentProps<typeof DropdownMenuRadioGroup>) {
  return <DropdownMenuRadioGroup {...props} />;
}

type PanelMenuContentProps = ComponentProps<typeof DropdownMenuContent> & {
  size?: "compact" | "default";
};

function PanelMenuContent({ className, size = "default", style, ...props }: PanelMenuContentProps) {
  return (
    <DropdownMenuContent
      className={cn(
        "flex flex-col gap-0.5 border border-pearl-border bg-menu-glass p-[5px] text-pearl-ink shadow-menu-glass backdrop-menu-glass contrast-more:border-pearl-ink contrast-more:bg-pearl-highlight",
        className,
      )}
      style={{ minWidth: 0, width: size === "compact" ? 92 : 152, ...style }}
      {...props}
    />
  );
}

const sharedItemStyles =
  "rounded-[6px] px-2 py-[7px] text-[9px] transition-colors hover:bg-pearl-ink/5 hover:text-pearl-ink active:bg-pearl-ink/10 focus:bg-pearl-ink/5 focus:text-pearl-ink data-[highlighted]:bg-pearl-ink/5 data-[highlighted]:text-pearl-ink";

type PanelMenuItemProps = ComponentProps<typeof DropdownMenuItem>;

function PanelMenuItem({ className, ...props }: PanelMenuItemProps) {
  return (
    <DropdownMenuItem
      className={cn(
        "cursor-pointer gap-2 data-disabled:pointer-events-auto data-disabled:cursor-not-allowed data-disabled:hover:bg-transparent data-disabled:hover:text-pearl-muted",
        sharedItemStyles,
        className,
      )}
      {...props}
    />
  );
}

type PanelMenuRadioItemProps = ComponentProps<typeof DropdownMenuRadioItem>;

function PanelMenuRadioItem({ className, ...props }: PanelMenuRadioItemProps) {
  return (
    <DropdownMenuRadioItem
      className={cn(
        "cursor-pointer data-[state=checked]:bg-action data-[state=checked]:font-semibold data-[state=checked]:text-accent-foreground data-[state=checked]:data-[highlighted]:bg-action data-[state=checked]:data-[highlighted]:text-accent-foreground [&_[data-slot=dropdown-menu-radio-item-indicator]]:hidden",
        sharedItemStyles,
        className,
      )}
      {...props}
    />
  );
}

export {
  PanelMenu,
  PanelMenuContent,
  PanelMenuItem,
  PanelMenuRadioGroup,
  PanelMenuRadioItem,
  PanelMenuTrigger,
};
