import { useId, useState } from "react";
import type {
  ProviderPresentation,
  TopModelUsage,
  UsageHistory,
  UsageHistoryDay,
  UsagePeriods,
  UsageTotal,
} from "@touchgrass/contracts";
import { PanelQuerySelector } from "./panel-query-selector";

type ChartPoint = UsageHistoryDay & { hour?: number };
type Period = "today" | "sevenDays" | "thirtyDays";
const periods = [
  { label: "Today", value: "today" },
  { label: "7 days", value: "sevenDays" },
  { label: "30 days", value: "thirtyDays" },
] as const;
const tokenFormatter = new Intl.NumberFormat("en", {
  notation: "compact",
  maximumFractionDigits: 1,
});
const dollars = new Intl.NumberFormat("en", { style: "currency", currency: "USD" });
const unavailable: UsageTotal = { availability: "unavailable" };
const tokens = (total: UsageTotal) =>
  total.availability === "unavailable" ? null : total.observedTokens;
const cost = (total: UsageTotal) =>
  total.availability === "unavailable" || total.apiEquivalentCostUsd == null
    ? "—"
    : `≈ ${dollars.format(total.apiEquivalentCostUsd)}`;
const modelName = (model: TopModelUsage | null | undefined) =>
  model ? (model.model ?? "Unknown model") : "—";
const dayLabel = (day: string) =>
  new Date(`${day}T00:00:00Z`).toLocaleDateString("en", {
    day: "numeric",
    month: "short",
    timeZone: "UTC",
  });

const pointLabel = (point: ChartPoint) =>
  point.hour === undefined
    ? `${dayLabel(point.day)} · UTC`
    : `${dayLabel(point.day)} · ${String(point.hour).padStart(2, "0")}:00–${String(point.hour + 1).padStart(2, "0")}:00 UTC`;

// Pricing and coverage remain part of the accessible description and point detail.
function evidence(total: UsageTotal) {
  if (total.availability === "unavailable") return "Usage not observed";
  return [
    total.availability === "stale" ? "Last recorded usage" : "Recorded usage",
    total.coverage === "partial" ? "Some usage may be missing" : null,
    total.apiEquivalentCostQuality === "modeled"
      ? `Cost modeled from ${Math.round(total.apiEquivalentCostCoveragePercent ?? 0)}% priced evidence`
      : total.apiEquivalentCostQuality === "local-only"
        ? "Cost from local pricing evidence"
        : null,
    total.trendPercent == null
      ? null
      : `${total.trendPercent > 0 ? "+" : ""}${Math.round(total.trendPercent * 10) / 10}% from the previous period`,
    total.apiEquivalentCostUsd == null
      ? "API equivalent unavailable"
      : "Estimated value at API rates",
  ]
    .filter(Boolean)
    .join(". ");
}

function UsageOverview({
  usage,
  providers = [],
  history,
  topModelUsage,
}: {
  usage: UsagePeriods;
  providers?: readonly ProviderPresentation[] | undefined;
  history?: UsageHistory | null | undefined;
  topModelUsage?: TopModelUsage | null | undefined;
}) {
  const [period, setPeriod] = useState<Period>("today");
  const [selectedProvider, setProvider] = useState("all");
  const provider = providers.find((p) => p.provider === selectedProvider);
  const scope = provider?.provider ?? "all";
  const selectedUsage = provider?.usage ?? usage;
  const selectedHistory = history?.scopes.find(
    (h) => h.provider === (scope === "all" ? null : scope),
  );
  const total = selectedUsage[period];
  const count = period === "today" ? 1 : period === "sevenDays" ? 7 : 30;
  const topModel =
    selectedHistory?.topModels[period] ??
    (period === "today"
      ? (provider?.topModelUsage ?? (scope === "all" ? topModelUsage : null))
      : null);
  const knownTokens = tokens(total);
  const scanStatus =
    selectedUsage[
      period === "today"
        ? "todayScanStatus"
        : period === "sevenDays"
          ? "sevenDayScanStatus"
          : "thirtyDayScanStatus"
    ] ?? selectedUsage.scanStatus;
  const points: ChartPoint[] =
    period === "today"
      ? (selectedHistory?.hours.map((hour) => ({ ...hour, day: history!.today })) ?? [])
      : (selectedHistory?.days.slice(-count) ?? []);
  const included = providers.filter((p) => scope === "all" || p.provider === scope);
  return (
    <section aria-label="Usage" className="usage-overview" data-slot="usage-overview">
      <div className="usage-summary">
        <div className="usage-total">
          <strong
            aria-label={
              knownTokens === null
                ? "Usage unavailable"
                : `${knownTokens.toLocaleString("en")} tokens`
            }
          >
            {knownTokens === null ? "—" : tokenFormatter.format(knownTokens)} <small>tokens</small>
          </strong>
          <span title={evidence(total)}>
            {cost(total) === "—" && scanStatus === "indexing" ? "Indexing…" : cost(total)} API
            equivalent
          </span>
        </div>
        <div className="flex shrink-0 items-center gap-0.5 text-pearl-muted">
          <PanelQuerySelector
            label="Usage period"
            options={periods}
            value={period}
            onValueChange={(value) => setPeriod(value as Period)}
          />
          <span aria-hidden="true" className="text-[8px]">
            ·
          </span>
          <PanelQuerySelector
            label="Usage provider"
            options={[
              { label: "Combined", value: "all" },
              ...providers.map((p) => ({ label: p.displayName, value: p.provider })),
            ]}
            value={scope}
            onValueChange={setProvider}
          />
        </div>
      </div>
      <UsageChart
        key={`${scope}-${period}-${history?.today ?? "unavailable"}`}
        points={points}
        hourly={period === "today"}
        history={history}
        providers={included}
        scope={scope}
        emptyLabel={scanStatus === "indexing" ? "Indexing…" : "Usage history is unavailable."}
      />
      {period === "today" &&
        selectedHistory &&
        !selectedHistory.hourlyMatchesTotal &&
        points.some((p) => tokens(p.total) !== null) && (
          <p className="usage-history-note">
            Hourly detail uses local records. Some usage may have no time data.
          </p>
        )}
      <div className="usage-footer">
        <span title="Most used model in the local records for this provider and period">
          Most used model <strong>{knownTokens === null ? "—" : modelName(topModel)}</strong>
        </span>
        {scope === "all" && included.length > 1 && (
          <div className="usage-legend" aria-label="Providers">
            {included.map((p) => (
              <span key={p.provider}>
                <i data-provider={p.provider} />
                {p.displayName}
              </span>
            ))}
          </div>
        )}
      </div>
    </section>
  );
}

function UsageChart({
  points,
  history,
  providers,
  scope,
  emptyLabel,
  hourly,
}: {
  points: ChartPoint[];
  hourly: boolean;
  history: UsageHistory | null | undefined;
  providers: readonly ProviderPresentation[];
  scope: string;
  emptyLabel: string;
}) {
  const [hovered, setHovered] = useState<number | null>(null);
  const [focused, setFocused] = useState<number | null>(null);
  const [dismissed, setDismissed] = useState(false);
  const tooltipId = useId();
  const active = dismissed ? null : (hovered ?? focused);
  const point = active === null ? undefined : points[active];
  const peak = Math.max(1, ...points.map((p) => tokens(p.total) ?? 0));
  const step = 10 ** Math.floor(Math.log10(peak));
  const maximum = Math.ceil(peak / step) * step;
  const providerDay = (provider: ProviderPresentation, point: ChartPoint) => {
    const data = history?.scopes.find((h) => h.provider === provider.provider);
    return (
      (point.hour === undefined
        ? data?.days.find((d) => d.day === point.day)
        : data?.hours.find((h) => h.hour === point.hour)
      )?.total ?? unavailable
    );
  };
  if (!points.some((p) => tokens(p.total) !== null))
    return <p className="usage-empty">{emptyLabel}</p>;
  return (
    <div className="usage-chart">
      <div className="usage-plot">
        <div className="usage-axis" aria-hidden="true">
          <span>{tokenFormatter.format(maximum)}</span>
          <span>{tokenFormatter.format(maximum / 2)}</span>
          <span>0</span>
        </div>
        <div
          className="usage-columns"
          style={{ gridTemplateColumns: `repeat(${points.length}, 1fr)` }}
          onPointerLeave={() => setHovered(null)}
        >
          {points.map((p, i) => {
            const value = tokens(p.total);
            return (
              <button
                key={`${p.day}-${p.hour ?? "day"}`}
                type="button"
                className="usage-column"
                aria-label={`${pointLabel(p)}: ${value === null ? "not observed" : `${value.toLocaleString("en")} tokens`}`}
                aria-describedby={active === i ? tooltipId : undefined}
                data-active={active === i}
                onPointerEnter={() => {
                  setHovered(i);
                  setDismissed(false);
                }}
                onFocus={() => {
                  setFocused(i);
                  setDismissed(false);
                }}
                onBlur={() => setFocused(null)}
                onClick={() => {
                  setFocused(i);
                  setDismissed(false);
                }}
                onKeyDown={(event) => {
                  if (event.key === "Escape") {
                    setDismissed(true);
                    event.stopPropagation();
                  }
                }}
              >
                {value === null ? (
                  <span className="usage-missing">—</span>
                ) : (
                  <span className="usage-stack" style={{ height: `${(value / maximum) * 100}%` }}>
                    {scope !== "all" || providers.length === 0 ? (
                      <span data-provider={scope} style={{ height: "100%" }} />
                    ) : (
                      providers.map((provider) => (
                        <span
                          key={provider.provider}
                          data-provider={provider.provider}
                          style={{
                            height: `${value === 0 ? 0 : ((tokens(providerDay(provider, p)) ?? 0) / value) * 100}%`,
                          }}
                        />
                      ))
                    )}
                  </span>
                )}
              </button>
            );
          })}
          {point && active !== null && (
            <div
              id={tooltipId}
              role="tooltip"
              className="usage-tooltip"
              style={{
                left: `clamp(0px, calc(${((active + 0.5) / points.length) * 100}% - 102px), calc(100% - 204px))`,
                bottom: `${((tokens(point.total) ?? 0) / maximum) * 100}%`,
              }}
            >
              <span className="usage-tooltip-date">{pointLabel(point)}</span>
              <strong>
                {tokens(point.total) === null ? "—" : tokenFormatter.format(tokens(point.total)!)}{" "}
                <small>tokens</small>
              </strong>
              <span>{cost(point.total)} API equivalent</span>
              <div className="usage-tooltip-model">
                <span>Most used model</span>
                <b>{modelName(point.topModelUsage)}</b>
              </div>
              {scope === "all" &&
                providers.length > 1 &&
                providers.map((provider) => {
                  const total = providerDay(provider, point);
                  const value = tokens(total);
                  return (
                    <div className="usage-tooltip-row" key={provider.provider}>
                      <span>
                        <i data-provider={provider.provider} />
                        {provider.displayName}
                      </span>
                      <b>
                        {value === null ? "—" : tokenFormatter.format(value)} · {cost(total)}
                      </b>
                    </div>
                  );
                })}
              <small className="usage-evidence">{evidence(point.total)}</small>
            </div>
          )}
        </div>
      </div>
      <div className="usage-dates">
        <span>{hourly ? "00:00 UTC" : `${dayLabel(points[0]!.day)} · UTC`}</span>
        <span>{hourly ? "Now" : dayLabel(points[points.length - 1]!.day)}</span>
      </div>
    </div>
  );
}

export { UsageOverview };
