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

function PanelMenuRadioGroup(
  props: ComponentProps<typeof DropdownMenuRadioGroup>,
) {
  return <DropdownMenuRadioGroup {...props} />;
}

type PanelMenuContentProps = ComponentProps<typeof DropdownMenuContent> & {
  size?: "menu" | "query";
};

function PanelMenuContent({
  className,
  size = "menu",
  style,
  ...props
}: PanelMenuContentProps) {
  return (
    <DropdownMenuContent
      className={cn(
        "backdrop-menu-glass flex flex-col gap-0.5 border border-cream-border bg-menu-glass p-[5px] text-cream-ink shadow-menu-glass contrast-more:border-cream-ink contrast-more:bg-cream-highlight",
        className,
      )}
      style={{ minWidth: 0, width: size === "query" ? 118 : 152, ...style }}
      {...props}
    />
  );
}

const sharedItemStyles =
  "rounded-[6px] px-2 py-[7px] text-[9px] hover:bg-action hover:text-accent-foreground focus:bg-action focus:text-accent-foreground data-[highlighted]:bg-action data-[highlighted]:text-accent-foreground";

type PanelMenuItemProps = ComponentProps<typeof DropdownMenuItem>;

function PanelMenuItem({ className, ...props }: PanelMenuItemProps) {
  return (
    <DropdownMenuItem
      className={cn(
        "cursor-pointer gap-2 data-disabled:pointer-events-auto data-disabled:cursor-not-allowed data-disabled:hover:bg-transparent data-disabled:hover:text-cream-muted",
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
        "cursor-pointer pr-7 data-[state=checked]:bg-action data-[state=checked]:font-semibold data-[state=checked]:text-accent-foreground",
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
