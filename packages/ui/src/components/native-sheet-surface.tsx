import type { ComponentProps } from "react";

import { cn } from "#lib/utils";
import { nativeSheetSurfaceClassName } from "#lib/native-sheet-styles";

type NativeSheetSurfaceProps = ComponentProps<"article">;

function NativeSheetSurface({ className, ...props }: NativeSheetSurfaceProps) {
  return (
    <article
      className={cn(nativeSheetSurfaceClassName, className)}
      data-slot="native-sheet-surface"
      {...props}
    />
  );
}

export { NativeSheetSurface };
export type { NativeSheetSurfaceProps };
