import type { ComponentProps } from "react";

import grassGlyph from "../assets/brand/grass-glyph-white.webp?url";
import { cn } from "#lib/utils";

type BrandMarkProps = ComponentProps<"img"> & {
  size?: "panel" | "sidebar";
  tone?: "color" | "ink" | "reversed";
};

type BrandProps = ComponentProps<"div"> & {
  markProps?: BrandMarkProps;
};

function BrandMark({ className, size = "panel", tone = "ink", ...props }: BrandMarkProps) {
  return (
    <img
      alt=""
      className={cn(
        "object-contain",
        size === "panel" ? "size-[18px]" : "size-[23px]",
        tone === "ink" && "brightness-0",
        tone === "reversed" && "brightness-0 invert",
        className,
      )}
      data-size={size}
      data-slot="brand-mark"
      data-tone={tone}
      src={grassGlyph}
      {...props}
    />
  );
}

function BrandWordmark({ className, ...props }: ComponentProps<"span">) {
  return (
    <span
      className={cn("flex items-baseline tracking-[-0.035em] whitespace-nowrap", className)}
      data-slot="brand-wordmark"
      {...props}
    >
      <strong className="text-[12px] font-bold text-pearl-ink">TouchGrass</strong>
      <strong className="relative text-[12px] font-bold text-pearl-ink after:absolute after:right-0 after:-bottom-0.5 after:left-0 after:h-0.5 after:rounded-full after:bg-primary after:content-['']">
        Bar
      </strong>
    </span>
  );
}

function Brand({ className, markProps, ...props }: BrandProps) {
  return (
    <div
      aria-label="TouchGrassBar"
      className={cn("flex items-center gap-2", className)}
      data-slot="brand"
      {...props}
    >
      <BrandMark {...markProps} />
      <BrandWordmark />
    </div>
  );
}

export { Brand, BrandMark, BrandWordmark };
export type { BrandMarkProps, BrandProps };
