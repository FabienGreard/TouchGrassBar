import type { ProviderSnapshot, QuotaLane } from "@touchgrass/contracts";
import { ProviderMark, QuotaProgress } from "@touchgrass/ui";

function quotaPercentage(lane: QuotaLane | null | undefined) {
  if (
    !lane?.allowance ||
    lane.remaining === null ||
    lane.remaining === undefined
  )
    return null;
  return Math.max(0, Math.min(100, (lane.remaining / lane.allowance) * 100));
}

function quotaLabel(
  lane: QuotaLane | null,
  availability: ProviderSnapshot["availability"],
) {
  if (!lane) return "Quota snapshot unavailable";
  const freshness = availability === "stale" ? " · stale" : "";
  if (!lane.resetAt) return `${lane.label}${freshness}`;

  const resetAt = new Date(lane.resetAt);
  if (Number.isNaN(resetAt.getTime()))
    return `${lane.label} · resets ${lane.resetAt}${freshness}`;

  const hours = resetAt.getUTCHours().toString().padStart(2, "0");
  const minutes = resetAt.getUTCMinutes().toString().padStart(2, "0");
  const time = `${hours}:${minutes}`;
  const weekday = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"][
    resetAt.getUTCDay()
  ];
  const resetLabel = /week/i.test(lane.label) ? `${weekday} ${time}` : time;

  return `${lane.label} · resets ${resetLabel}${freshness}`;
}

function quotaAriaLabel(
  label: string,
  availability: ProviderSnapshot["availability"],
  percentage: number | null,
) {
  return `${label} quota ${
    percentage === null
      ? "unavailable"
      : `${availability}, ${Math.round(percentage)} percent remaining`
  }`;
}

function orderedQuotaLanes(provider: ProviderSnapshot) {
  if (provider.availability === "unavailable") return provider.quotaLanes;
  const weeklyLane = provider.quotaLanes.find((lane) =>
    /week/i.test(lane.label),
  );
  if (!weeklyLane) return provider.quotaLanes;
  return [
    weeklyLane,
    ...provider.quotaLanes.filter((lane) => lane !== weeklyLane),
  ];
}

function ProviderQuotaLane({
  lane,
  availability,
  provider,
}: {
  lane: QuotaLane;
  availability: ProviderSnapshot["availability"];
  provider: ProviderSnapshot["provider"];
}) {
  const percentage = quotaPercentage(lane);

  return (
    <div
      className="mt-2.5 grid grid-cols-[1fr_auto] items-center gap-x-2 gap-y-1"
      data-slot="provider-quota-lane"
    >
      <small className="truncate text-[8px] text-pearl-muted contrast-more:text-pearl-ink">
        {quotaLabel(lane, availability)}
      </small>
      <strong
        className={
          percentage === null
            ? "text-[9px] text-usage-unavailable"
            : "text-[9px]"
        }
      >
        {percentage === null ? "—" : `${Math.round(percentage)}%`}
      </strong>
      <div className="col-span-2">
        <QuotaProgress
          aria-label={quotaAriaLabel(
            lane.label,
            availability,
            percentage,
          )}
          provider={provider}
          size="secondary"
          value={percentage}
        />
      </div>
    </div>
  );
}

function ProviderCard({ provider }: { provider: ProviderSnapshot }) {
  const label = provider.provider === "codex" ? "Codex" : "Claude";
  const lanes = orderedQuotaLanes(provider);
  const primaryLane = lanes[0] ?? null;
  const secondaryLanes = lanes.slice(1);
  const percentage = quotaPercentage(primaryLane);

  return (
    <section
      aria-labelledby={`${provider.provider}-heading`}
      className="border-b border-pearl-line bg-provider-row px-4 py-[15px] contrast-more:border-pearl-ink contrast-more:bg-pearl-highlight"
      data-provider-availability={provider.availability}
    >
      <header className="grid grid-cols-[auto_1fr_auto] items-center gap-2.5">
        <span className="grid h-[29px] w-[29px] place-items-center overflow-hidden rounded-[7px] border border-input bg-pearl-control shadow-control contrast-more:border-pearl-ink">
          <ProviderMark provider={provider.provider} />
        </span>
        <span className="flex min-w-0 flex-col gap-0.5">
          <strong className="text-[13px]" id={`${provider.provider}-heading`}>
            {label}
          </strong>
          <small className="truncate text-[10px] text-pearl-muted contrast-more:text-pearl-ink">
            {quotaLabel(primaryLane, provider.availability)}
          </small>
        </span>
        <strong
          aria-label={quotaAriaLabel(
            label,
            provider.availability,
            percentage,
          )}
          className={
            percentage === null
              ? "text-[21px] text-usage-unavailable"
              : "text-[21px]"
          }
        >
          {percentage === null ? "—" : `${Math.round(percentage)}%`}
        </strong>
      </header>

      <div className="mt-3">
        <QuotaProgress
          aria-label={quotaAriaLabel(
            label,
            provider.availability,
            percentage,
          )}
          provider={provider.provider}
          value={percentage}
        />
      </div>
      {secondaryLanes.map((lane) => (
        <ProviderQuotaLane
          availability={provider.availability}
          key={lane.label}
          lane={lane}
          provider={provider.provider}
        />
      ))}
    </section>
  );
}

export { ProviderCard };
