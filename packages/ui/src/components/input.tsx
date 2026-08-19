import * as React from "react";

import { cn } from "#lib/utils";

function Input({ className, type, ...props }: React.ComponentProps<"input">) {
  return (
    <input
      className={cn(
        "h-9 w-full min-w-0 rounded-[8px] border border-input bg-white px-3 text-[11px] shadow-control transition-[border-color,box-shadow] outline-none placeholder:text-pearl-muted/60 focus:border-pearl-focus focus:ring-3 focus:ring-pearl-focus/25 disabled:cursor-not-allowed disabled:opacity-50 aria-invalid:border-destructive aria-invalid:ring-3 aria-invalid:ring-destructive/20 contrast-more:border-pearl-ink",
        className,
      )}
      data-slot="input"
      type={type}
      {...props}
    />
  );
}

export { Input };
export type InputProps = React.ComponentProps<"input">;
