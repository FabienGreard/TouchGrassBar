import type { ComponentProps } from "react";

import { cn } from "#lib/utils";

type PanelShellProps = ComponentProps<"main"> & {
  glass?: boolean;
};

function PanelShell({
  className,
  glass = false,
  ...props
}: PanelShellProps) {
  return (
    <main
      className={cn(
        "relative w-[402px] max-w-full overflow-hidden rounded-panel border border-cream-border bg-panel-glass text-cream-ink shadow-panel-glass contrast-more:border-cream-ink contrast-more:bg-cream-highlight contrast-more:shadow-none",
        glass && "backdrop-panel-glass",
        className,
      )}
      data-glass={glass || undefined}
      data-slot="panel-shell"
      {...props}
    />
  );
}

export { PanelShell };
export type { PanelShellProps };
