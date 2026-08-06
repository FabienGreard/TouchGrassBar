import type { UsagePeriods, UsageTotal } from "@touchgrass/contracts";
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
  trend?: string | undefined;
  trendDescription?: string | undefined;
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

function evidenceLabel(
  total: Exclude<UsageTotal, { availability: "unavailable" }>,
  scanStatus: UsagePeriods["scanStatus"],
) {
  const basis =
    total.evidenceBasis === "provider-reported"
      ? "Provider reported"
      : "Locally derived";
  const coverage = total.coverage === "complete" ? "Complete" : "Partial";
  const scan =
    scanStatus === "indexing"
      ? " · Indexing"
      : scanStatus === "unavailable"
        ? " · Scan unavailable"
        : "";
  return `${basis} · ${coverage}${scan}`;
}

function costLabel(total: Exclude<UsageTotal, { availability: "unavailable" }>) {
  if (
    total.apiEquivalentCostUsd === null ||
    total.apiEquivalentCostUsd === undefined ||
    !total.apiEquivalentCostBasis ||
    !total.apiEquivalentCostQuality
  ) {
    return "API equivalent unavailable";
  }
  const price = `≈ ${currencyFormatter.format(total.apiEquivalentCostUsd)}`;
  if (total.apiEquivalentCostQuality === "reconciled")
    return `${price} · Reconciled`;
  if (total.apiEquivalentCostQuality === "local-only")
    return `${price} · Local only`;
  const coverage = total.apiEquivalentCostCoveragePercent;
  if (
    coverage === null ||
    coverage === undefined ||
    !Number.isFinite(coverage) ||
    coverage < 0 ||
    coverage > 100
  ) {
    return "API equivalent unavailable";
  }
  return `${price} · Modeled ${Math.round(coverage)}%`;
}

function trendPresentation(
  total: UsageTotal,
  gaugeFill: number | undefined,
  comparison: string,
): UsageMetricPresentation | undefined {
  if (total.availability === "unavailable" || gaugeFill === undefined)
    return undefined;
  if (total.trendPercent === null || total.trendPercent === undefined)
    return { gaugeFill };
  const rounded = Math.round(total.trendPercent * 10) / 10;
  const trend = `${rounded > 0 ? "+" : ""}${rounded}%`;
  const direction = rounded > 0 ? "Up" : rounded < 0 ? "Down" : "No change";
  const magnitude = Math.abs(rounded);
  return {
    gaugeFill,
    trend,
    trendDescription:
      direction === "No change"
        ? `No change from ${comparison}`
        : `${direction} ${magnitude} percent from ${comparison}`,
  };
}

function relativeGaugeFills(usage: UsagePeriods) {
  const values = [usage.today, usage.sevenDays, usage.thirtyDays].map((total) =>
    total.availability === "unavailable" ? undefined : total.observedTokens,
  );
  const maximum = Math.max(0, ...values.filter((value) => value !== undefined));
  return values.map((value) =>
    value === undefined ? undefined : maximum === 0 ? 0 : Math.round((value / maximum) * 100),
  );
}

function UsageMetric({
  label,
  presentation,
  tone,
  total,
  scanStatus,
}: {
  label: string;
  presentation?: UsageMetricPresentation | undefined;
  tone: GaugeTone;
  total: UsageTotal;
  scanStatus: UsagePeriods["scanStatus"];
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
    <MetricCard className="pb-[27px]">
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
      <MetricCardDetail
        className="flex flex-col overflow-visible whitespace-normal"
        tone="positive"
      >
        <span>{costLabel(total)}</span>
        <span className="text-pearl-muted contrast-more:text-pearl-ink">
          Price basis: {total.apiEquivalentCostBasis ?? "Unavailable"}
        </span>
        <span className="text-pearl-muted contrast-more:text-pearl-ink">
          {evidenceLabel(total, scanStatus)}
        </span>
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

function UsageOverview({
  presentation,
  usage,
}: {
  presentation?: UsagePresentation | undefined;
  usage: UsagePeriods;
}) {
  const [todayGauge, sevenDayGauge, thirtyDayGauge] = relativeGaugeFills(usage);
  const resolvedPresentation = {
    today:
      presentation?.today ??
      trendPresentation(usage.today, todayGauge, "the previous day"),
    sevenDays:
      presentation?.sevenDays ??
      trendPresentation(
        usage.sevenDays,
        sevenDayGauge,
        "the previous 7 days",
      ),
    thirtyDays:
      presentation?.thirtyDays ??
      trendPresentation(
        usage.thirtyDays,
        thirtyDayGauge,
        "the previous 30 days",
      ),
  };
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
            presentation={resolvedPresentation.today}
            tone="today"
            total={usage.today}
            scanStatus={usage.scanStatus}
          />
          <UsageMetric
            label="7 days"
            presentation={resolvedPresentation.sevenDays}
            tone="week"
            total={usage.sevenDays}
            scanStatus={usage.scanStatus}
          />
          <UsageMetric
            label="30 days"
            presentation={resolvedPresentation.thirtyDays}
            tone="month"
            total={usage.thirtyDays}
            scanStatus={usage.scanStatus}
          />
        </MetricCardGroup>
      </div>
    </section>
  );
}

export { UsageOverview };
export type { UsageMetricPresentation, UsagePresentation };
