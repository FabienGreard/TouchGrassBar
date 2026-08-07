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

function evidenceDescription(
  total: Exclude<UsageTotal, { availability: "unavailable" }>,
) {
  const basis =
    total.evidenceBasis === "provider-reported"
      ? "provider-reported token evidence"
      : total.evidenceBasis === "locally-derived"
        ? "locally observed token evidence"
        : "mixed token evidence";
  const coverage =
    total.coverage === "complete"
      ? "complete period coverage"
      : "partial period coverage";
  const costEvidence =
    total.apiEquivalentCostQuality === "reconciled"
      ? "pricing detail covers the reported tokens"
      : total.apiEquivalentCostQuality === "modeled" &&
          total.apiEquivalentCostCoveragePercent !== null &&
          total.apiEquivalentCostCoveragePercent !== undefined
        ? `cost modeled from ${Math.round(total.apiEquivalentCostCoveragePercent)} percent priced evidence`
        : total.apiEquivalentCostQuality === "local-only"
          ? "cost estimated from local pricing evidence"
          : "cost evidence not ready";
  return `${basis}, ${coverage}, ${costEvidence}`;
}

function costPresentation(
  total: Exclude<UsageTotal, { availability: "unavailable" }>,
  scanStatus: UsagePeriods["scanStatus"],
) {
  const evidence = evidenceDescription(total);
  if (
    total.apiEquivalentCostUsd !== null &&
    total.apiEquivalentCostUsd !== undefined
  ) {
    const label = `≈ ${currencyFormatter.format(total.apiEquivalentCostUsd)}`;
    return {
      accessibleLabel: total.apiEquivalentCostBasis
        ? `${label}, ${evidence}, pricing basis ${total.apiEquivalentCostBasis}${scanStatus === "indexing" ? ", indexing" : ""}`
        : `${label}, ${evidence}${scanStatus === "indexing" ? ", indexing" : ""}`,
      label,
      ready: true,
    };
  }
  if (scanStatus === "indexing") {
    return {
      accessibleLabel: `API equivalent indexing, ${evidence}`,
      label: "Indexing…",
      ready: false,
    };
  }
  return {
    accessibleLabel: `API equivalent not ready, ${evidence}`,
    label: "—",
    ready: false,
  };
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
  const values = [usage.today, usage.sevenDays, usage.thirtyDays].map(
    (total) =>
      total.availability === "unavailable" ? undefined : total.observedTokens,
  );
  const maximum = Math.max(0, ...values.filter((value) => value !== undefined));
  return values.map((value) =>
    value === undefined
      ? undefined
      : maximum === 0
        ? 0
        : Math.round((value / maximum) * 100),
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
    const detail = scanStatus === "indexing" ? "Indexing…" : "Not observed";
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
        <MetricCardDetail>{detail}</MetricCardDetail>
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

  const cost = costPresentation(total, scanStatus);

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
      <MetricCardDetail
        aria-label={cost.accessibleLabel}
        className="inline-flex items-center gap-0.5"
        tone={cost.ready ? "positive" : "muted"}
      >
        <span className="overflow-hidden text-ellipsis">{cost.label}</span>
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
      trendPresentation(usage.sevenDays, sevenDayGauge, "the previous 7 days"),
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
            scanStatus={usage.todayScanStatus ?? usage.scanStatus}
          />
          <UsageMetric
            label="7 days"
            presentation={resolvedPresentation.sevenDays}
            tone="week"
            total={usage.sevenDays}
            scanStatus={usage.sevenDayScanStatus ?? usage.scanStatus}
          />
          <UsageMetric
            label="30 days"
            presentation={resolvedPresentation.thirtyDays}
            tone="month"
            total={usage.thirtyDays}
            scanStatus={usage.thirtyDayScanStatus ?? usage.scanStatus}
          />
        </MetricCardGroup>
      </div>
    </section>
  );
}

export { UsageOverview };
export type { UsageMetricPresentation, UsagePresentation };
