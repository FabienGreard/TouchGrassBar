import * as React from "react";
import { cva, type VariantProps } from "class-variance-authority";
import { Slot } from "radix-ui";

import { cn } from "#lib/utils";

const buttonVariants = cva(
  "group/button inline-flex shrink-0 items-center justify-center rounded-lg border border-transparent bg-clip-padding text-sm font-medium whitespace-nowrap transition-all outline-none select-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50 disabled:pointer-events-none disabled:opacity-50 aria-invalid:border-destructive aria-invalid:ring-3 aria-invalid:ring-destructive/20 dark:aria-invalid:border-destructive/50 dark:aria-invalid:ring-destructive/40 [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4",
  {
    variants: {
      variant: {
        action:
          "border-board-tab-active-border bg-action text-accent-foreground shadow-action hover:brightness-[1.03] aria-expanded:brightness-[1.03]",
        quiet:
          "cursor-pointer border-transparent bg-transparent text-cream-muted shadow-none hover:bg-cream-ink/5 hover:text-cream-ink aria-expanded:bg-cream-ink/5 aria-expanded:text-cream-ink focus-visible:border-transparent focus-visible:bg-cream-ink/5 focus-visible:text-cream-ink focus-visible:ring-0",
      },
      size: {
        default:
          "h-auto gap-1 rounded-[8px] px-[13px] py-[7px] text-[8px] font-semibold",
        identity:
          "h-5 max-w-[142px] gap-1 rounded-[5px] px-1.5 font-mono text-[7px] font-medium leading-none",
        quiet:
          "h-5 gap-1 rounded-[6px] px-1.5 text-[8px] font-semibold leading-none [&_svg:not([class*='size-'])]:size-3",
        icon: "h-[28px] w-[30px] rounded-[8px] p-1",
        sheet:
          "h-9 w-full gap-1.5 rounded-[8px] px-3 text-[13px] font-semibold",
      },
    },
    defaultVariants: {
      variant: "action",
      size: "default",
    },
  },
);

function Button({
  className,
  variant = "action",
  size = "default",
  asChild = false,
  ...props
}: React.ComponentProps<"button"> &
  VariantProps<typeof buttonVariants> & {
    asChild?: boolean;
  }) {
  const Comp = asChild ? Slot.Root : "button";

  return (
    <Comp
      data-slot="button"
      data-variant={variant}
      data-size={size}
      className={cn(buttonVariants({ variant, size, className }))}
      {...props}
    />
  );
}

export { Button };
