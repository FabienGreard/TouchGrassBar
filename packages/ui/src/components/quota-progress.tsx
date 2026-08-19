import { Progress as ProgressPrimitive } from "radix-ui";
import type { ComponentProps, CSSProperties } from "react";

import { cn } from "#lib/utils";

type QuotaProvider = "claude" | "codex";
type QuotaProgressSize = "primary" | "secondary";

type QuotaProgressProps = Omit<ComponentProps<typeof ProgressPrimitive.Root>, "style" | "value"> & {
  provider: QuotaProvider;
  size?: QuotaProgressSize;
  value: number | null;
};

function normalizeValue(value: number | null) {
  if (value === null) return null;
  return Math.max(0, Math.min(100, value));
}

function quotaStyle(provider: QuotaProvider, value: number | null): CSSProperties {
  if (value === null) {
    return {
      "--quota-fill": "var(--usage-unavailable)",
      "--quota-glow": "transparent",
    } as CSSProperties;
  }

  const amount = Math.round(value);
  return {
    "--quota-fill": `color-mix(in oklab, var(--quota-${provider}-low) ${100 - amount}%, var(--quota-${provider}-high) ${amount}%)`,
    "--quota-glow": `color-mix(in srgb, var(--quota-${provider}-high) 42%, transparent)`,
  } as CSSProperties;
}

function QuotaProgress({
  className,
  provider,
  size = "primary",
  value,
  ...props
}: QuotaProgressProps) {
  const normalizedValue = normalizeValue(value);

  return (
    <ProgressPrimitive.Root
      className={cn(
        "relative flex w-full items-center overflow-x-hidden rounded-full bg-progress-track shadow-progress-track",
        size === "primary" ? "h-[5px]" : "h-[4px]",
        className,
      )}
      data-quota-tone={normalizedValue === null ? "unavailable" : provider}
      data-quota-value={normalizedValue === null ? undefined : Math.round(normalizedValue)}
      data-size={size}
      data-slot="quota-progress"
      style={quotaStyle(provider, normalizedValue)}
      value={normalizedValue}
      {...props}
    >
      <ProgressPrimitive.Indicator
        className="size-full flex-1 bg-[var(--quota-fill)] shadow-[0_0_10px_var(--quota-glow)] transition-transform motion-reduce:transition-none"
        data-slot="quota-progress-indicator"
        style={{
          transform: `translateX(-${100 - (normalizedValue ?? 0)}%)`,
        }}
      />
    </ProgressPrimitive.Root>
  );
}

export { QuotaProgress };
export type { QuotaProgressProps, QuotaProgressSize, QuotaProvider };
