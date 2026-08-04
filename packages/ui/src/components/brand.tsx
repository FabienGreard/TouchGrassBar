import type { ComponentProps } from "react";

import lilyGlyph from "../assets/brand/lily-glyph-split-decay.png";
import { cn } from "#lib/utils";

type BrandMarkProps = ComponentProps<"img"> & {
  size?: "panel" | "sidebar";
  tone?: "ink" | "reversed";
};

function BrandMark({
  className,
  size = "panel",
  tone = "ink",
  ...props
}: BrandMarkProps) {
  return (
    <img
      alt=""
      className={cn(
        "object-contain brightness-0",
        size === "panel" ? "size-[18px]" : "size-[23px]",
        tone === "reversed" && "invert",
        className,
      )}
      data-size={size}
      data-slot="brand-mark"
      data-tone={tone}
      src={lilyGlyph}
      {...props}
    />
  );
}

function BrandWordmark({ className, ...props }: ComponentProps<"span">) {
  return (
    <span
      className={cn(
        "flex items-baseline whitespace-nowrap tracking-[-0.035em]",
        className,
      )}
      data-slot="brand-wordmark"
      {...props}
    >
      <strong className="text-[12px] font-bold text-cream-ink">
        TouchGrass
      </strong>
      <strong className="text-[12px] font-bold text-primary">Bar</strong>
    </span>
  );
}

function Brand({ className, ...props }: ComponentProps<"div">) {
  return (
    <div
      aria-label="TouchGrassBar"
      className={cn("flex items-center gap-2", className)}
      data-slot="brand"
      {...props}
    >
      <BrandMark />
      <BrandWordmark />
    </div>
  );
}

export { Brand, BrandMark, BrandWordmark };
export type { BrandMarkProps };
