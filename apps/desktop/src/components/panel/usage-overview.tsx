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

import type { UsageMetricPreview, UsagePreview } from "@/previewFixtures";

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
  preview,
  tone,
  total,
}: {
  label: string;
  preview?: UsageMetricPreview | undefined;
  tone: GaugeTone;
  total: UsageTotal;
}) {
  if (total.availability === "unavailable") {
    return (
      <MetricCard>
        <MetricCardLabel>{label}</MetricCardLabel>
        <MetricCardTrend
          aria-label={preview?.trendDescription ?? `${label} trend unavailable`}
          tone={getMetricTrendTone(preview?.trend)}
        >
          {preview?.trend ?? "—"}
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
            preview
              ? `${label} development preview gauge ${preview.gaugeFill} percent`
              : `${label} usage gauge unavailable`
          }
          {...(preview ? { fill: preview.gaugeFill } : {})}
          tone={tone}
        />
      </MetricCard>
    );
  }

  return (
    <MetricCard>
      <MetricCardLabel>{label}</MetricCardLabel>
      <MetricCardTrend
        aria-label={preview?.trendDescription ?? `${label} trend unavailable`}
        tone={getMetricTrendTone(preview?.trend)}
      >
        {preview?.trend ?? "—"}
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
          preview
            ? `${label} development preview gauge ${preview.gaugeFill} percent`
            : `${label} usage gauge unavailable`
        }
        {...(preview ? { fill: preview.gaugeFill } : {})}
        tone={tone}
      />
    </MetricCard>
  );
}

type CodexUsage = SanitizedDesktopState["usage"]["codex"];

function UsageOverview({
  preview,
  usage,
}: {
  preview?: UsagePreview | undefined;
  usage: CodexUsage;
}) {
  return (
    <section
      aria-labelledby="observed-usage-heading"
      className="border-b border-cream-line bg-usage-surface px-4 py-[13px] contrast-more:border-cream-ink contrast-more:bg-cream-highlight"
      data-preview-fixture={preview ? "usage" : undefined}
    >
      <header className="flex items-center justify-between">
        <h2 className="m-0 text-[10px]" id="observed-usage-heading">
          Observed tokens
        </h2>
        <small className="text-[8px] text-cream-muted contrast-more:text-cream-ink">
          API equivalent
        </small>
      </header>
      <div className="mt-2">
        <MetricCardGroup>
        <UsageMetric
          label="Today"
          preview={preview?.today}
          tone="today"
          total={usage.today}
        />
        <UsageMetric
          label="7 days"
          preview={preview?.sevenDays}
          tone="week"
          total={usage.sevenDays}
        />
        <UsageMetric
          label="30 days"
          preview={preview?.thirtyDays}
          tone="month"
          total={usage.thirtyDays}
        />
        </MetricCardGroup>
      </div>
    </section>
  );
}

export { UsageOverview };
