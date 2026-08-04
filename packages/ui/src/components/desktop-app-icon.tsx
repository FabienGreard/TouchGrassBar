import type { ComponentProps } from "react";

import desktopAppIcon from "../assets/brand/glass-lily-frosted-pearl.png";
import { cn } from "#lib/utils";

type DesktopAppIconProps = Omit<ComponentProps<"img">, "src"> & {
  size?: "large" | "sidebar";
};

function DesktopAppIcon({
  alt = "",
  className,
  draggable = false,
  size = "sidebar",
  ...props
}: DesktopAppIconProps) {
  return (
    <img
      alt={alt}
      className={cn(
        "shrink-0 object-contain",
        size === "sidebar" ? "size-9" : "size-16",
        className,
      )}
      data-size={size}
      data-slot="desktop-app-icon"
      draggable={draggable}
      src={desktopAppIcon}
      {...props}
    />
  );
}

export { DesktopAppIcon };
export type { DesktopAppIconProps };
