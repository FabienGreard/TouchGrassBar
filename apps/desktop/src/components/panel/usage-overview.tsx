import type { SanitizedDesktopState, UsageTotal } from "@touchgrass/contracts";
import {
  MetricCard,
  MetricCardDetail,
  MetricCardGauge,
  MetricCardGroup,
  MetricCardLabel,
  MetricCardTrend,
  MetricCardValue,
  getMetricTrendTone,
} from "@touchgrass/ui";

type UsageMetricPresentation = {
  gaugeFill: number;
  trend: string;
  trendDescription: string;
};

type UsagePresentation = {
  sevenDays: UsageMetricPresentation;
  thirtyDays: UsageMetricPresentation;
  today: UsageMetricPresentation;
};

const tokenFormatter = new Intl.NumberFormat("en", {
  maximumFractionDigits: 1,
  notation: "compact",
});
const currencyFormatter = new Intl.NumberFormat("en", {
  currency: "USD",
  maximumFractionDigits: 2,
  minimumFractionDigits: 2,
  style: "currency",
});

type GaugeTone = "month" | "today" | "week";

function UsageMetric({
  label,
  presentation,
  tone,
  total,
}: {
  label: string;
  presentation?: UsageMetricPresentation | undefined;
  tone: GaugeTone;
  total: UsageTotal;
}) {
  if (total.availability === "unavailable") {
    return (
      <MetricCard>
        <MetricCardLabel>{label}</MetricCardLabel>
        <MetricCardTrend
          aria-label={
            presentation?.trendDescription ?? `${label} trend unavailable`
          }
          tone={getMetricTrendTone(presentation?.trend)}
        >
          {presentation?.trend ?? "—"}
        </MetricCardTrend>
        <MetricCardValue
          aria-label={`${label} usage unavailable`}
          tone="unavailable"
        >
          —
        </MetricCardValue>
        <MetricCardDetail>Not observed</MetricCardDetail>
        <MetricCardGauge
          aria-label={
            presentation
              ? `${label} usage gauge ${presentation.gaugeFill} percent`
              : `${label} usage gauge unavailable`
          }
          {...(presentation ? { fill: presentation.gaugeFill } : {})}
          tone={tone}
        />
      </MetricCard>
    );
  }

  return (
    <MetricCard>
      <MetricCardLabel>{label}</MetricCardLabel>
      <MetricCardTrend
        aria-label={
          presentation?.trendDescription ?? `${label} trend unavailable`
        }
        tone={getMetricTrendTone(presentation?.trend)}
      >
        {presentation?.trend ?? "—"}
      </MetricCardTrend>
      <MetricCardValue>
        {tokenFormatter.format(total.observedTokens)}
      </MetricCardValue>
      <MetricCardDetail tone="positive">
        {total.apiEquivalentCostUsd === null ||
        total.apiEquivalentCostUsd === undefined
          ? "API equivalent unavailable"
          : `≈ ${currencyFormatter.format(total.apiEquivalentCostUsd)}`}
      </MetricCardDetail>
      <MetricCardGauge
        aria-label={
          presentation
            ? `${label} usage gauge ${presentation.gaugeFill} percent`
            : `${label} usage gauge unavailable`
        }
        {...(presentation ? { fill: presentation.gaugeFill } : {})}
        tone={tone}
      />
    </MetricCard>
  );
}

type CodexUsage = SanitizedDesktopState["usage"]["codex"];

function UsageOverview({
  presentation,
  usage,
}: {
  presentation?: UsagePresentation | undefined;
  usage: CodexUsage;
}) {
  return (
    <section
      aria-labelledby="observed-usage-heading"
      className="border-b border-pearl-line bg-usage-surface px-4 py-[13px] contrast-more:border-pearl-ink contrast-more:bg-pearl-highlight"
    >
      <header className="flex items-center justify-between">
        <h2 className="m-0 text-[10px]" id="observed-usage-heading">
          Observed tokens
        </h2>
        <small className="text-[8px] text-pearl-muted contrast-more:text-pearl-ink">
          API equivalent
        </small>
      </header>
      <div className="mt-2">
        <MetricCardGroup>
          <UsageMetric
            label="Today"
            presentation={presentation?.today}
            tone="today"
            total={usage.today}
          />
          <UsageMetric
            label="7 days"
            presentation={presentation?.sevenDays}
            tone="week"
            total={usage.sevenDays}
          />
          <UsageMetric
            label="30 days"
            presentation={presentation?.thirtyDays}
            tone="month"
            total={usage.thirtyDays}
          />
        </MetricCardGroup>
      </div>
    </section>
  );
}

export { UsageOverview };
export type { UsageMetricPresentation, UsagePresentation };
