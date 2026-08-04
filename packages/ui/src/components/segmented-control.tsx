import type { ComponentProps } from "react";

import { cn } from "#lib/utils";
import { Tabs, TabsList, TabsTrigger } from "../internal/tabs";

function SegmentedControl({
  "aria-label": ariaLabel,
  children,
  className,
  ...props
}: ComponentProps<typeof Tabs>) {
  return (
    <Tabs
      className={cn("w-fit gap-0", className)}
      data-slot="segmented-control"
      {...props}
    >
      <TabsList
        aria-label={ariaLabel}
        className="h-auto gap-0.5 rounded-[11px] border border-cream-ink/[0.07] bg-board-tab-surface p-[3px] shadow-progress-track contrast-more:border-cream-ink"
      >
        {children}
      </TabsList>
    </Tabs>
  );
}

type SegmentedControlItemProps = ComponentProps<typeof TabsTrigger>;

function SegmentedControlItem({
  className,
  ...props
}: SegmentedControlItemProps) {
  return (
    <TabsTrigger
      className={cn(
        "h-auto min-w-[82px] rounded-[8px] border-transparent bg-transparent px-[13px] py-[7px] text-[8px] font-semibold text-cream-muted shadow-none hover:bg-cream-ink/5 hover:text-cream-ink focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50 focus-visible:outline-none data-[state=active]:border-board-tab-active-border data-[state=active]:bg-action data-[state=active]:text-accent-foreground data-[state=active]:shadow-action data-[state=active]:hover:bg-action data-[state=active]:hover:text-accent-foreground data-[state=active]:focus-visible:border-ring",
        className,
      )}
      data-segmented-control-item=""
      {...props}
    />
  );
}

export { SegmentedControl, SegmentedControlItem };
