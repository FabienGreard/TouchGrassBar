import type { ProviderPresentation, ProviderSnapshot, QuotaLane } from "@touchgrass/contracts";

import { ProviderMark } from "./provider-mark";
import { QuotaProgress } from "./quota-progress";

type ProviderCardProps = {
  presentation: ProviderPresentation;
  referenceTime?: string | undefined;
  timeZone?: string | undefined;
};

function resetTimeFormatter(timeZone?: string) {
  return new Intl.DateTimeFormat("en-GB", {
    hour: "2-digit",
    hourCycle: "h23",
    minute: "2-digit",
    timeZone,
  });
}

function exactResetDate(resetAt: Date, timeZone?: string) {
  const parts = new Intl.DateTimeFormat("en-GB", {
    day: "numeric",
    hour: "2-digit",
    hourCycle: "h23",
    minute: "2-digit",
    month: "short",
    timeZone,
    weekday: "short",
  }).formatToParts(resetAt);
  const part = (type: Intl.DateTimeFormatPartTypes) =>
    parts.find((candidate) => candidate.type === type)?.value ?? "";

  return `${part("weekday")} ${part("day")} ${part("month")}, ${part("hour")}:${part("minute")}`;
}

function quotaPercentage(lane: QuotaLane | null | undefined) {
  if (!lane?.allowance || lane.remaining === null || lane.remaining === undefined) return null;
  return Math.max(0, Math.min(100, (lane.remaining / lane.allowance) * 100));
}

function quotaLabel(lane: QuotaLane | null, referenceTime?: string, timeZone?: string) {
  if (!lane) return "Quota snapshot unavailable";
  if (!lane.resetAt) return lane.label;

  const resetAt = new Date(lane.resetAt);
  if (Number.isNaN(resetAt.getTime())) return `${lane.label} · resets ${lane.resetAt}`;

  const referenceTimeMilliseconds = referenceTime ? new Date(referenceTime).getTime() : Date.now();
  const referenceMilliseconds = Number.isNaN(referenceTimeMilliseconds)
    ? Date.now()
    : referenceTimeMilliseconds;
  if (resetAt.getTime() <= referenceMilliseconds) {
    const reset = /week/i.test(lane.label)
      ? exactResetDate(resetAt, timeZone)
      : resetTimeFormatter(timeZone).format(resetAt);
    return `${lane.label} · reset ${reset}`;
  }

  const remainingMilliseconds = resetAt.getTime() - referenceMilliseconds;
  const totalMinutes = Math.floor(remainingMilliseconds / 60_000);
  const totalHours = Math.floor(totalMinutes / 60);
  const days = Math.floor(totalHours / 24);
  const hoursLeft = totalHours % 24;
  const minutesLeft = totalMinutes % 60;
  const timeLeft =
    days > 0
      ? `${days}d ${hoursLeft}h`
      : totalHours > 0
        ? `${totalHours}h ${minutesLeft}m`
        : `${minutesLeft}m`;

  const time = resetTimeFormatter(timeZone).format(resetAt);
  if (!/week/i.test(lane.label)) return `${lane.label} · resets ${time}`;

  const exactReset = exactResetDate(resetAt, timeZone);
  return `${lane.label} · ${timeLeft} left · ${exactReset}`;
}

function quotaAriaLabel(label: string, percentage: number | null) {
  return `${label} quota ${
    percentage === null ? "unavailable" : `${Math.round(percentage)} percent remaining`
  }`;
}

function orderedQuotaLanes(provider: ProviderSnapshot) {
  if (provider.availability === "unavailable") return provider.quotaLanes;
  const weeklyLane = provider.quotaLanes.find((lane) => /week/i.test(lane.label));
  if (!weeklyLane) return provider.quotaLanes;
  return [weeklyLane, ...provider.quotaLanes.filter((lane) => lane !== weeklyLane)];
}

function ProviderQuotaLane({
  lane,
  provider,
  referenceTime,
  timeZone,
}: {
  lane: QuotaLane;
  provider: ProviderSnapshot["provider"];
  referenceTime?: string | undefined;
  timeZone?: string | undefined;
}) {
  const percentage = quotaPercentage(lane);

  return (
    <div
      className="mt-2.5 grid grid-cols-[1fr_auto] items-center gap-x-2 gap-y-1"
      data-slot="provider-quota-lane"
    >
      <small className="truncate text-[8px] text-pearl-muted contrast-more:text-pearl-ink">
        {quotaLabel(lane, referenceTime, timeZone)}
      </small>
      <strong className={percentage === null ? "text-[9px] text-usage-unavailable" : "text-[9px]"}>
        {percentage === null ? "—" : `${Math.round(percentage)}%`}
      </strong>
      <div className="col-span-2">
        <QuotaProgress
          aria-label={quotaAriaLabel(lane.label, percentage)}
          provider={provider}
          size="secondary"
          value={percentage}
        />
      </div>
    </div>
  );
}

function ProviderCard({ presentation, referenceTime, timeZone }: ProviderCardProps) {
  const { displayName: label, quota: provider } = presentation;
  const hasCachedQuotaOrObservedUsage =
    provider.availability !== "unavailable" ||
    [presentation.usage.today, presentation.usage.sevenDays, presentation.usage.thirtyDays].some(
      (usage) => usage.availability !== "unavailable",
    );
  const loading = presentation.usage.scanStatus === "indexing" && !hasCachedQuotaOrObservedUsage;
  const lanes = orderedQuotaLanes(provider);
  const primaryLane = lanes[0] ?? null;
  const secondaryLanes = lanes.slice(1);
  const percentage = quotaPercentage(primaryLane);

  return (
    <section
      aria-busy={loading || undefined}
      aria-labelledby={`${provider.provider}-heading`}
      className={`border-b border-pearl-line bg-provider-row px-4 py-[15px] contrast-more:border-pearl-ink contrast-more:bg-pearl-highlight ${
        loading ? "pointer-events-none animate-pulse motion-reduce:animate-none" : ""
      }`}
      data-provider-availability={provider.availability}
      data-provider-presence={presentation.presence}
    >
      {loading ? <span className="sr-only">Refreshing {label}…</span> : null}
      <header className="grid grid-cols-[auto_1fr_auto] items-center gap-2.5">
        <span className="grid h-[29px] w-[29px] place-items-center overflow-hidden rounded-[7px] border border-input bg-pearl-control shadow-control contrast-more:border-pearl-ink">
          <ProviderMark provider={provider.provider} />
        </span>
        <span className="flex min-w-0 flex-col gap-0.5">
          <strong className="text-[13px]" id={`${provider.provider}-heading`}>
            {label}
          </strong>
          <small className="truncate text-[10px] text-pearl-muted contrast-more:text-pearl-ink">
            {quotaLabel(primaryLane, referenceTime, timeZone)}
          </small>
        </span>
        <strong
          aria-label={quotaAriaLabel(label, percentage)}
          className={percentage === null ? "text-[21px] text-usage-unavailable" : "text-[21px]"}
        >
          {percentage === null ? "—" : `${Math.round(percentage)}%`}
        </strong>
      </header>

      <div className="mt-3">
        <QuotaProgress
          aria-label={quotaAriaLabel(label, percentage)}
          provider={provider.provider}
          value={percentage}
        />
      </div>
      {secondaryLanes.map((lane) => (
        <ProviderQuotaLane
          key={lane.label}
          lane={lane}
          provider={provider.provider}
          referenceTime={referenceTime}
          timeZone={timeZone}
        />
      ))}
    </section>
  );
}

export { ProviderCard };
