import type { ComponentProps } from "react";

import { cn } from "#lib/utils";

function NativeWindow({ className, ...props }: ComponentProps<"main">) {
  return (
    <main
      className={cn(
        "grid min-h-screen overflow-hidden bg-native-sheet text-sheet-ink min-[740px]:grid-cols-[190px_minmax(0,1fr)]",
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
        "hidden min-h-screen flex-col border-r border-sheet-line bg-sheet-sidebar px-3.5 py-7 min-[740px]:flex",
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
      className={cn("min-w-0 px-12 py-11 min-[740px]:px-14", className)}
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

function NativeWindowNavItem({ className, ...props }: ComponentProps<"span">) {
  return (
    <span
      className={cn(
        "rounded-[7px] px-2.5 py-2 text-sheet-muted aria-[current=page]:bg-sheet-active aria-[current=page]:font-semibold aria-[current=page]:text-sheet-ink aria-disabled:opacity-45",
        className,
      )}
      data-slot="native-window-nav-item"
      {...props}
    />
  );
}

export {
  NativeWindow,
  NativeWindowContent,
  NativeWindowNav,
  NativeWindowNavItem,
  NativeWindowSidebar,
};
