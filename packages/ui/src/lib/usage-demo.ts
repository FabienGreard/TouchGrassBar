/** Synthetic daily records for the landing page, Storybook, and browser fixtures only. */
import type {
  ProviderPresentation,
  TopModelUsage,
  UsageHistory,
  UsageHistoryDay,
  UsageTotal,
} from "@touchgrass/contracts";
const tokens = (total: UsageTotal) =>
  total.availability === "unavailable" ? 0 : total.observedTokens;
const cost = (total: UsageTotal) =>
  total.availability === "unavailable" ? 0 : (total.apiEquivalentCostUsd ?? 0);
const top = (days: UsageHistoryDay[]): TopModelUsage | null => {
  const models = new Map<string | null | undefined, number>();
  for (const day of days)
    if (day.topModelUsage)
      models.set(
        day.topModelUsage.model,
        (models.get(day.topModelUsage.model) ?? 0) + day.topModelUsage.observedTokens,
      );
  const winner = [...models].sort((a, b) => b[1] - a[1])[0];
  return winner ? { model: winner[0] ?? null, observedTokens: winner[1] } : null;
};
export function createUsageDemoHistory(
  providers: readonly ProviderPresentation[],
  referenceAt: string,
): UsageHistory {
  const today = referenceAt.slice(0, 10);
  const hourCount = new Date(referenceAt).getUTCHours() + 1;
  const scopes = providers.flatMap((p) => {
    if (p.usage.today.availability === "unavailable") return [];
    const days: UsageHistoryDay[] = Array.from({ length: 30 }, (_, index) => {
      const ago = 29 - index;
      const date = new Date(`${today}T00:00:00Z`);
      date.setUTCDate(date.getUTCDate() - ago);
      const current = ago === 0 ? p.usage.today : ago < 7 ? p.usage.sevenDays : p.usage.thirtyDays;
      const previous = ago < 7 ? p.usage.today : p.usage.sevenDays;
      const length = ago < 7 ? 6 : 23;
      const part = ago < 7 ? 6 - ago : 29 - ago;
      const distribute = (amount: number) =>
        Math.floor((amount * (part + 1)) / length) - Math.floor((amount * part) / length);
      const observedTokens =
        ago === 0 ? tokens(current) : distribute(Math.max(0, tokens(current) - tokens(previous)));
      const apiEquivalentCostUsd =
        ago === 0
          ? cost(current)
          : distribute(Math.round(Math.max(0, cost(current) - cost(previous)) * 100)) / 100;
      const model =
        p.provider === "codex"
          ? ago < 7
            ? "GPT 5.6 Sol"
            : "GPT 5.5"
          : ago === 0
            ? "Claude Opus 4.6"
            : "Claude Sonnet 4.6";
      return {
        day: date.toISOString().slice(0, 10),
        total:
          current.availability === "unavailable"
            ? current
            : {
                ...current,
                observedTokens,
                apiEquivalentCostUsd,
                observedAt: date.toISOString(),
              },
        topModelUsage: observedTokens ? { model, observedTokens } : null,
      };
    });
    const todayPoint = days[29]!;
    const weights = Array.from({ length: hourCount }, (_, hour) =>
      hour < 7 ? 0 : 1 + ((hour * 7) % 9),
    );
    if (weights.reduce((a, b) => a + b, 0) === 0) weights[hourCount - 1] = 1;
    const weightTotal = weights.reduce((a, b) => a + b, 0);
    const hours = weights.map((weight, hour) => {
      const previous = weights.slice(0, hour).reduce((a, b) => a + b, 0);
      const share = (amount: number) =>
        Math.floor((amount * (previous + weight)) / weightTotal) -
        Math.floor((amount * previous) / weightTotal);
      const observedTokens = share(tokens(todayPoint.total));
      return {
        hour,
        total:
          todayPoint.total.availability === "unavailable"
            ? todayPoint.total
            : {
                ...todayPoint.total,
                trendPercent: null,
                trendPreviousTokens: null,
                observedTokens,
                apiEquivalentCostUsd: share(Math.round(cost(todayPoint.total) * 100)) / 100,
              },
        topModelUsage: observedTokens
          ? { model: todayPoint.topModelUsage?.model ?? null, observedTokens }
          : null,
      };
    });
    return [
      {
        provider: p.provider,
        days,
        hours,
        hourlyMatchesTotal: true,
        topModels: {
          today: top(days.slice(-1)),
          sevenDays: top(days.slice(-7)),
          thirtyDays: top(days),
        },
      },
    ];
  });
  const days: UsageHistoryDay[] = Array.from({ length: 30 }, (_, index) => {
    const parts = scopes.map((s) => s.days[index]!);
    const reference = parts[0];
    const allModels = parts
      .flatMap((p) => (p.topModelUsage ? [p.topModelUsage] : []))
      .sort((a, b) => b.observedTokens - a.observedTokens);
    return {
      day: reference?.day ?? today,
      total:
        !reference || reference.total.availability === "unavailable"
          ? { availability: "unavailable" }
          : {
              ...reference.total,
              observedTokens: parts.reduce((sum, p) => sum + tokens(p.total), 0),
              apiEquivalentCostUsd: parts.reduce((sum, p) => sum + cost(p.total), 0),
            },
      topModelUsage: allModels[0] ?? null,
    };
  });
  const combinedTop = (count: number) => top(scopes.flatMap((s) => s.days.slice(-count)));
  return {
    today,
    scopes: [
      {
        provider: null,
        days,
        hourlyMatchesTotal: true,
        hours: Array.from({ length: hourCount }, (_, hour) => {
          const parts = scopes.map((s) => s.hours[hour]!);
          const reference = parts[0];
          const total =
            !reference || reference.total.availability === "unavailable"
              ? { availability: "unavailable" as const }
              : {
                  ...reference.total,
                  observedTokens: parts.reduce((sum, p) => sum + tokens(p.total), 0),
                  apiEquivalentCostUsd: parts.reduce((sum, p) => sum + cost(p.total), 0),
                };
          const model =
            parts
              .flatMap((p) => (p.topModelUsage ? [p.topModelUsage] : []))
              .sort((a, b) => b.observedTokens - a.observedTokens)[0] ?? null;
          return { hour, total, topModelUsage: model };
        }),
        topModels: {
          today: combinedTop(1),
          sevenDays: combinedTop(7),
          thirtyDays: combinedTop(30),
        },
      },
      ...scopes,
    ],
  };
}
