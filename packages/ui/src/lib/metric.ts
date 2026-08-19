type MetricCardTrendTone = "negative" | "neutral" | "positive";

function getMetricTrendTone(trend: string | null | undefined): MetricCardTrendTone {
  if (!trend) return "neutral";

  const value = Number.parseFloat(trend.trim().replace("−", "-"));
  if (value < 0) return "negative";
  if (value > 0) return "positive";
  return "neutral";
}

export { getMetricTrendTone };
export type { MetricCardTrendTone };
