import { Slot } from "radix-ui";
import { cva, type VariantProps } from "class-variance-authority";
import type { ComponentProps } from "react";

import { cn } from "#lib/utils";

function NativeWindow({ className, ...props }: ComponentProps<"main">) {
  return (
    <main
      className={cn(
        "grid min-h-screen overflow-hidden bg-native-sheet text-sheet-ink min-[680px]:grid-cols-[190px_minmax(0,1fr)]",
        className,
      )}
      data-slot="native-window"
      {...props}
    />
  );
}

function NativeWindowSidebar({ className, ...props }: ComponentProps<"aside">) {
  return (
    <aside
      className={cn(
        "hidden min-h-screen flex-col border-r border-sheet-line bg-sheet-sidebar px-3.5 py-7 min-[680px]:flex",
        className,
      )}
      data-slot="native-window-sidebar"
      {...props}
    />
  );
}

function NativeWindowContent({
  className,
  ...props
}: ComponentProps<"section">) {
  return (
    <section
      className={cn(
        "h-screen min-h-0 min-w-0 overflow-y-auto overscroll-contain px-12 py-11 [scrollbar-color:var(--sheet-muted)_transparent] [scrollbar-gutter:stable] [scrollbar-width:thin] min-[680px]:px-14 [&::-webkit-scrollbar]:w-1 [&::-webkit-scrollbar-thumb]:rounded-full [&::-webkit-scrollbar-thumb]:bg-sheet-ink/15",
        className,
      )}
      data-slot="native-window-content"
      {...props}
    />
  );
}

function NativeWindowNav({ className, ...props }: ComponentProps<"nav">) {
  return (
    <nav
      className={cn("mt-6 grid gap-1 text-[12px]", className)}
      data-slot="native-window-nav"
      {...props}
    />
  );
}

const nativeWindowNavItemVariants = cva(
  "group/native-window-nav-item rounded-[8px] text-sheet-muted outline-none select-none transition-colors hover:bg-pearl-ink/5 hover:text-sheet-ink active:bg-pearl-ink/10 focus-visible:ring-3 focus-visible:ring-ring/50 aria-disabled:pointer-events-none aria-disabled:opacity-45",
  {
    variants: {
      variant: {
        section:
          "px-3 py-2.5 aria-[current=page]:bg-action aria-[current=page]:font-semibold aria-[current=page]:text-accent-foreground aria-[current=page]:hover:bg-action aria-[current=page]:hover:text-accent-foreground aria-[current=page]:active:bg-action",
        step: "grid grid-cols-[22px_1fr] items-center gap-2 px-2 py-2.5 text-left text-[11px] aria-[current=step]:bg-sheet-active aria-[current=step]:font-semibold aria-[current=step]:text-sheet-ink aria-[current=step]:hover:bg-sheet-active aria-[current=step]:hover:text-sheet-ink aria-[current=step]:active:bg-sheet-active",
      },
    },
    defaultVariants: {
      variant: "section",
    },
  },
);

type NativeWindowNavItemProps = ComponentProps<"span"> &
  VariantProps<typeof nativeWindowNavItemVariants> & { asChild?: boolean };

function NativeWindowNavItem({
  asChild = false,
  className,
  variant = "section",
  ...props
}: NativeWindowNavItemProps) {
  const Comp = asChild ? Slot.Root : "span";

  return (
    <Comp
      className={cn(nativeWindowNavItemVariants({ variant }), className)}
      data-slot="native-window-nav-item"
      data-variant={variant}
      {...props}
    />
  );
}

function NativeWindowNavStepMarker({
  className,
  complete = false,
  ...props
}: ComponentProps<"span"> & { complete?: boolean }) {
  return (
    <span
      className={cn(
        "grid size-5 place-items-center rounded-full border border-sheet-line text-[8px] transition-colors group-aria-[current=step]/native-window-nav-item:border-sheet-ink",
        complete && "border-action bg-action text-accent-foreground",
        className,
      )}
      data-complete={complete || undefined}
      data-slot="native-window-nav-step-marker"
      {...props}
    />
  );
}

export {
  NativeWindow,
  NativeWindowContent,
  NativeWindowNav,
  NativeWindowNavItem,
  NativeWindowNavStepMarker,
  NativeWindowSidebar,
};
