import type { ComponentProps } from "react";

import type { MetricCardTrendTone } from "#lib/metric";
import { cn } from "#lib/utils";

function MetricCard({ className, ...props }: ComponentProps<"article">) {
  return (
    <article
      className={cn("relative flex min-w-0 flex-col px-2.5 pt-[9px] pb-[18px]", className)}
      data-slot="metric-card"
      {...props}
    />
  );
}

function MetricCardGroup({ className, ...props }: ComponentProps<"div">) {
  return (
    <div
      className={cn(
        "grid grid-cols-3 divide-x divide-pearl-line rounded-[10px] border border-pearl-line bg-pearl-surface shadow-surface contrast-more:divide-pearl-ink contrast-more:border-pearl-ink",
        className,
      )}
      data-slot="metric-card-group"
      {...props}
    />
  );
}

function MetricCardLabel({ className, ...props }: ComponentProps<"small">) {
  return (
    <small
      className={cn("text-[8px] text-pearl-muted contrast-more:text-pearl-ink", className)}
      data-slot="metric-card-label"
      {...props}
    />
  );
}

function MetricCardValue({
  className,
  tone = "default",
  ...props
}: ComponentProps<"strong"> & {
  tone?: "default" | "unavailable";
}) {
  return (
    <strong
      className={cn(
        "my-0.5 text-[16px] tracking-[-0.04em]",
        tone === "unavailable" && "text-usage-unavailable",
        className,
      )}
      data-slot="metric-card-value"
      data-tone={tone}
      {...props}
    />
  );
}

function MetricCardDetail({
  className,
  tone = "muted",
  ...props
}: ComponentProps<"span"> & {
  tone?: "muted" | "positive";
}) {
  return (
    <span
      className={cn(
        "overflow-hidden text-[7px] text-ellipsis whitespace-nowrap text-pearl-muted contrast-more:text-pearl-ink",
        tone === "positive" && "text-positive",
        className,
      )}
      data-slot="metric-card-detail"
      data-tone={tone}
      {...props}
    />
  );
}

function MetricCardTrend({
  className,
  tone = "neutral",
  ...props
}: ComponentProps<"span"> & { tone?: MetricCardTrendTone }) {
  return (
    <span
      className={cn(
        "absolute top-2 right-2.5 text-[8px] font-bold",
        tone === "positive" && "text-positive",
        tone === "negative" && "text-destructive",
        tone === "neutral" && "text-pearl-muted",
        className,
      )}
      data-slot="metric-card-trend"
      data-tone={tone}
      {...props}
    />
  );
}

const gaugeTones = {
  month: "bg-gauge-month",
  today: "bg-gauge-today",
  week: "bg-gauge-week",
} as const;

type MetricCardGaugeProps = ComponentProps<"span"> & {
  fill?: number;
  tone: keyof typeof gaugeTones;
};

function MetricCardGauge({ className, fill, tone, ...props }: MetricCardGaugeProps) {
  const normalizedFill = fill === undefined ? 0 : Math.max(0, Math.min(100, fill));

  return (
    <span
      className={cn(
        "absolute right-2.5 bottom-[7px] left-2.5 block h-[5px] overflow-hidden rounded-full border border-pearl-line-soft bg-progress-track shadow-progress-track",
        className,
      )}
      data-slot="metric-gauge"
      {...props}
    >
      <span
        aria-hidden="true"
        className={cn("block h-full rounded-full", gaugeTones[tone])}
        style={{ width: `${normalizedFill}%` }}
      />
    </span>
  );
}

export {
  MetricCard,
  MetricCardDetail,
  MetricCardGauge,
  MetricCardGroup,
  MetricCardLabel,
  MetricCardTrend,
  MetricCardValue,
};
